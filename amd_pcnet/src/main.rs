#![no_std]
#![no_main]

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

pub mod bitfields;

use core::{
    iter::Cycle,
    mem::size_of,
    ops::{ControlFlow, Range},
    ptr::null_mut,
    slice,
};

use alloc::{sync::Arc, vec::Vec};
use kernel_sys::{
    syscall::{sys_exit, sys_map, sys_process_spawn_thread, sys_yield},
    types::{Hid, KernelObjectType, MapMemoryFlags, SyscallResult},
};
use spin::Mutex;
use x86_64::instructions::port::Port;

use kernel_userspace::{
    backoff_sleep,
    channel::Channel,
    handle::Handle,
    interrupt::Interrupt,
    net::PhysicalNet,
    pci::PCIDevice,
    process::get_handle,
    service::{deserialize, serialize, Service},
    INT_PCI,
};

use self::bitfields::InitBlock;

pub enum SendError {
    BufferFull,
}

const IP_ADDR: u32 = 100 << 24 | 1 << 16 | 168 << 8 | 192;

const BUFFER_ENTRY_SIZE: u32 = 2048;
const BUFFER_SIZE_MASK: u32 = 0xF000 | (0xFFF & (1 + !(BUFFER_ENTRY_SIZE)));
const SEND_BUFFER_CNT_LOG: u8 = 3;
const RECV_BUFFER_CNT_LOG: u8 = 3;
const SEND_BUFFER_CNT: usize = 2usize.pow(SEND_BUFFER_CNT_LOG as u32);
const RECV_BUFFER_CNT: usize = 2usize.pow(RECV_BUFFER_CNT_LOG as u32);

#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
struct BufferDescriptor {
    address: u32,
    flags: u32,
    flags_2: u32,
    avail: u32,
}

#[unsafe(export_name = "_start")]
pub extern "C" fn main() {
    let pci_ref = unsafe { Handle::from_id(Hid::from_usize(2).unwrap()) };
    assert_eq!(
        kernel_sys::syscall::sys_object_type(*pci_ref).unwrap(),
        KernelObjectType::Channel
    );
    let pci_device = Channel::from_handle(pci_ref);

    let pcnet = Arc::new(Mutex::new(
        PCNET::new(PCIDevice {
            device_service: pci_device,
        })
        .unwrap(),
    ));

    sys_process_spawn_thread({
        let pcnet = pcnet.clone();
        move || {
            let interrupts = Channel::from_handle(backoff_sleep(|| get_handle("INTERRUPTS")));

            let (_, mut handles) = interrupts.call_val::<1, _, ()>(&INT_PCI, &[]).unwrap();
            let pci_ev = Interrupt::from_handle(handles.pop().unwrap());

            loop {
                pci_ev.wait().assert_ok();
                pcnet.lock().interrupt_handler();
            }
        }
    });

    let mut buffer = Vec::new();
    Service::new(
        "PCNET",
        || (),
        |handle, ()| {
            let mut handles = match handle.read::<1>(&mut buffer, true, false) {
                Ok(h) => h,
                e => {
                    println!("Error: {e:?}");
                    return ControlFlow::Break(());
                }
            };

            match deserialize(&buffer).unwrap() {
                PhysicalNet::MacAddrGet => {
                    if !handles.is_empty() {
                        println!("Bad amount of handles");
                        return ControlFlow::Break(());
                    }
                    let resp = pcnet.lock().read_mac_addr();
                    let resp = serialize(&resp, &mut buffer);
                    handle.write(&resp, &[]).assert_ok();
                }
                PhysicalNet::SendPacket(packet) => {
                    if !handles.is_empty() {
                        println!("Bad amount of handles");
                        return ControlFlow::Break(());
                    }
                    // Keep trying to send
                    while pcnet.lock().send_packet(packet).is_err() {
                        sys_yield()
                    }
                    handle.write(&[], &[]).assert_ok();
                }
                PhysicalNet::ListenToPackets => {
                    if handles.len() != 1 {
                        println!("Bad amount of handles");
                        return ControlFlow::Break(());
                    }
                    pcnet
                        .lock()
                        .listeners
                        .push(Channel::from_handle(handles.pop().unwrap()));
                    handle.write(&[], &[]).assert_ok();
                }
            };
            ControlFlow::Continue(())
        },
    )
    .run();
}

pub struct PCNETIOPort(u16);

impl PCNETIOPort {
    fn write_rap_32(&mut self, val: u32) {
        let mut port = Port::new(self.0 + 0x14);
        unsafe { port.write(val) }
    }

    fn read_csr_32(&mut self, csr_no: u32) -> u32 {
        self.write_rap_32(csr_no);
        let mut port = Port::new(self.0 + 0x10);
        unsafe { port.read() }
    }

    fn write_csr_32(&mut self, csr_no: u32, val: u32) {
        self.write_rap_32(csr_no);
        let mut port = Port::new(self.0 + 0x10);
        unsafe { port.write(val) }
    }

    fn read_bcr_32(&mut self, bcr: u32) -> u32 {
        self.write_rap_32(bcr);
        let mut port = Port::new(self.0 + 0x1C);
        unsafe { port.read() }
    }

    fn write_bcr_32(&mut self, bcr: u32, val: u32) {
        self.write_rap_32(bcr);
        let mut port = Port::new(self.0 + 0x1C);
        unsafe { port.write(val) }
    }

    fn reset_device(&mut self) {
        let mut reset_port_32: Port<u32> = Port::new(self.0 + 0x18);
        let mut reset_port_16: Port<u32> = Port::new(self.0 + 0x14);
        // Reset to defaults
        unsafe {
            reset_port_32.read();
            reset_port_16.read();
        }
        // We need to wait 1ms
        sys_yield();
        // 32 bit mode
        let mut data_register: Port<u32> = Port::new(self.0 + 0x10);
        unsafe {
            data_register.write(0);
        }
        // SWSTYLE (32 bit buffers)
        let mut csr58 = self.read_csr_32(58);
        csr58 &= 0xFF00;
        csr58 |= 2;
        self.write_csr_32(58, csr58);

        // Asel
        let mut bcr_2 = self.read_bcr_32(2);
        bcr_2 |= 2;
        self.write_bcr_32(2, bcr_2);
    }

    fn read_mac_addr(&mut self) -> u64 {
        let mut mac_address: Port<u32> = Port::new(self.0);
        let mut mac_address2: Port<u32> = Port::new(self.0 + 0x4);
        unsafe {
            let mac = mac_address.read() as u64;
            let mac2 = mac_address2.read() as u64 & 0xFFFF;
            mac2 << 32 | mac
        }
    }
}

#[allow(dead_code)]
pub struct PCNET<'b> {
    io: PCNETIOPort,
    init_block: &'b mut InitBlock,
    send_buffer_desc: &'b mut [BufferDescriptor],
    send_buffer_pos: Cycle<Range<usize>>,
    recv_buffer_desc: &'b mut [BufferDescriptor],
    revc_buffer_pos: Cycle<Range<usize>>,
    owned_pages: Vec<u32>,
    listeners: Vec<Channel>,
}

fn mmap_page32() -> u32 {
    unsafe {
        sys_map(
            null_mut(),
            0x1000,
            MapMemoryFlags::WRITEABLE | MapMemoryFlags::ALLOC_32BITS,
        )
        .unwrap() as u32
    }
}

impl PCNET<'_> {
    fn new(pci_device: kernel_userspace::pci::PCIDevice) -> Option<Self> {
        let common_header = kernel_userspace::pci::PCIHeaderCommon {
            device: Arc::new(Mutex::new(pci_device)),
        };
        // Ensure device is actually supported
        if !(common_header.get_vendor_id() == 0x1022 && common_header.get_device_id() == 0x2000) {
            return None;
        };

        let pci_device = unsafe { common_header.get_as_header0() };

        let port_base: u16 = pci_device.get_port_base().unwrap().try_into().unwrap();
        let mut port = PCNETIOPort(port_base);

        port.reset_device();

        let mac = port.read_mac_addr();

        let header_mem_size: usize = size_of::<InitBlock>()
            + size_of::<BufferDescriptor>() * (RECV_BUFFER_CNT + SEND_BUFFER_CNT);

        assert!(header_mem_size <= 0x1000);

        let mut owned_pages = Vec::new();

        let (init_block, send_buffer_desc, recv_buffer_desc) = unsafe {
            // Allocate (identity mapped) page below 4gb location.
            let buffer = mmap_page32();

            let buffer_start = buffer;
            owned_pages.push(buffer);

            let mut buffer_start = buffer_start as *const u8;

            // Init block
            let init_block = &mut *(buffer_start as *mut InitBlock);

            buffer_start = buffer_start.add(size_of::<InitBlock>());

            let send_buffer_desc =
                slice::from_raw_parts_mut(buffer_start as *mut BufferDescriptor, SEND_BUFFER_CNT);

            buffer_start = buffer_start.add(size_of::<[BufferDescriptor; SEND_BUFFER_CNT]>());
            let recv_buffer_desc =
                slice::from_raw_parts_mut(buffer_start as *mut BufferDescriptor, RECV_BUFFER_CNT);
            (init_block, send_buffer_desc, recv_buffer_desc)
        };

        // init_block.set_mode(0x8000); // promiscours mode = true;
        init_block.set_mode(0); // promiscours mode = false;
        init_block.set_num_send_buffers(SEND_BUFFER_CNT_LOG);
        init_block.set_num_recv_buffers(RECV_BUFFER_CNT_LOG);
        init_block.set_physical_address(mac);
        init_block.set_logical_address(IP_ADDR.into());
        init_block
            .set_send_buffer_desc_addr(&send_buffer_desc[0] as *const BufferDescriptor as u32);

        init_block
            .set_recv_buffer_desc_addr(&recv_buffer_desc[0] as *const BufferDescriptor as u32);

        // Alloc buffer each 2 buffer
        for i in (0..SEND_BUFFER_CNT).step_by(2) {
            // Allocate page below 4gb location.
            let buffer = mmap_page32();
            owned_pages.push(buffer);

            send_buffer_desc[i].address = buffer;
            send_buffer_desc[i].flags = BUFFER_SIZE_MASK;
            send_buffer_desc[i + 1].address = buffer + 2048;
            send_buffer_desc[i + 1].flags = BUFFER_SIZE_MASK;
        }
        // Alloc buffer each 2 buffer
        for i in (0..RECV_BUFFER_CNT).step_by(2) {
            // Allocate page below 4gb location.
            let buffer = mmap_page32();
            owned_pages.push(buffer);

            recv_buffer_desc[i].address = buffer;
            recv_buffer_desc[i].flags = BUFFER_SIZE_MASK | 0x80000000;
            recv_buffer_desc[i + 1].address = buffer + 2048;
            recv_buffer_desc[i + 1].flags = BUFFER_SIZE_MASK | 0x80000000;
        }

        let init_block_addr = init_block as *const InitBlock as u32;

        let mut this = Self {
            io: port,
            init_block,
            send_buffer_pos: (0..send_buffer_desc.len()).cycle(),
            send_buffer_desc,
            revc_buffer_pos: (0..recv_buffer_desc.len()).cycle(),
            recv_buffer_desc,
            owned_pages,
            listeners: Vec::new(),
        };

        // Write regs
        this.io.write_csr_32(1, init_block_addr);
        this.io.write_csr_32(2, init_block_addr >> 16);

        // Set init
        this.io.write_csr_32(0, 1);
        while this.io.read_csr_32(0) & (1 << 7) == 0 {
            println!("... {}", this.io.read_csr_32(0));
            sys_yield();
        }
        assert!(this.io.read_csr_32(0) == 0b110000001); // IDON + INTR + INIT

        // Start card
        this.io.write_csr_32(0, 2 | 0x40);
        assert!(this.io.read_csr_32(0) == 0b111110011); // IDON + INTR + RXON + TXON + STRT + INIT + IENA

        // Clear any interrupts the card send (INIT)
        this.interrupt_handler();
        println!("PCNET inited");
        Some(this)
    }

    fn interrupt_handler(&mut self) {
        // Stop interrupts
        let tmp = self.io.read_csr_32(0);
        self.io.write_csr_32(0, tmp & !0x40);
        if tmp & 0x8000 > 0 {
            println!("AMD am79c973 ERROR")
        }
        if tmp & 0x2000 > 0 {
            println!("AMD am79c973 COLLISION ERROR")
        }
        if tmp & 0x1000 > 0 {
            println!("AMD am79c973 MISSED FRAME")
        }
        if tmp & 0x800 > 0 {
            println!("AMD am79c973 MEMORY ERROR")
        }
        if tmp & 0x400 > 0 {
            println!("AMD am79c973 DATA RECEIVED");
            self.receive();
        } else {
            // TODO: QEMU For some reason doesn't assert the bitflags in csr 0 to saw what caused the interrupts
            // At least it sends a PCI interrupt so just check the buffers whenever there is an interrupt.
            println!("AMD am79c973 Checking receive buffers.");
            self.receive();
        }
        if tmp & 0x200 > 0 {
            println!("AMD am79c973 DATA SENT")
        }
        if tmp & 0x100 > 0 {
            println!("AMD am79c973 INIT DONE")
        }
        // Start interrupts again
        self.io.write_csr_32(0, 0x40);
    }
}

impl PCNET<'_> {
    fn send_packet(&mut self, data: &[u8]) -> Result<(), SendError> {
        for buffer in self
            .send_buffer_pos
            .by_ref()
            .take(self.send_buffer_desc.len())
        {
            let buffer_desc = &mut self.send_buffer_desc[buffer];
            // Find a buffer which we own
            if buffer_desc.flags & 0x80000000 == 0 {
                let send_buffer = unsafe {
                    slice::from_raw_parts_mut(
                        buffer_desc.address as *mut u8,
                        BUFFER_ENTRY_SIZE as usize,
                    )
                };
                send_buffer[..data.len()].clone_from_slice(data);

                buffer_desc.avail = 0;
                buffer_desc.flags_2 = 0;
                // Then length is twos complement of bytes
                buffer_desc.flags = 0x8300F000 | ((!data.len() + 1) as u16 as u32);

                // Set TDMD
                let tmp = self.io.read_csr_32(0);
                self.io.write_csr_32(0, tmp | 0x8);
                return Ok(());
            }
        }
        Err(SendError::BufferFull)
    }

    fn read_mac_addr(&mut self) -> u64 {
        self.io.read_mac_addr()
    }

    pub fn receive(&mut self) {
        for buffer in self
            .revc_buffer_pos
            .by_ref()
            .take(self.recv_buffer_desc.len())
        {
            let buffer_desc = &mut self.recv_buffer_desc[buffer];
            let flags = buffer_desc.flags;
            if flags & 0x80000000 == 0 {
                if flags & 0x40000000 == 0 && flags & 0x03000000 > 0 {
                    let size: usize = buffer_desc.flags_2 as usize & 0xFFFF;
                    let packet =
                        unsafe { slice::from_raw_parts(buffer_desc.address as *const u8, size) };
                    self.listeners
                        .retain(|l| l.write(packet, &[]) == SyscallResult::Ok);
                }
                buffer_desc.flags = 0x80000000 | BUFFER_SIZE_MASK;
                buffer_desc.flags_2 = 0;
            }
        }
    }
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    sys_exit()
}
