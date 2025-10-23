use core::mem::MaybeUninit;
use core::ptr::null_mut;
use core::time::Duration;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use num_traits::FromPrimitive;

use super::raw::syscall::*;
use super::raw::types::*;
use super::types::*;

#[inline]
pub fn sys_echo(val: usize) -> usize {
    unsafe { raw_sys_echo(val) }
}

#[inline]
pub fn sys_yield() {
    unsafe { raw_sys_yield() }
}

#[inline]
pub fn sys_sleep(time: Duration) -> Duration {
    unsafe { Duration::from_millis(raw_sys_sleep(time.as_millis() as u64)) }
}

#[inline]
pub fn sys_exit() -> ! {
    unsafe { raw_sys_exit() }
}

/// Map syscall
///
/// # Safety
///
/// The caller must ensure the arguments are correct as this will perform mapping into the vmspace
#[inline]
pub unsafe fn sys_map(
    vmo: Option<Hid>,
    flags: VMMapFlags,
    hint: vaddr_t,
    length: usize,
) -> Result<vaddr_t, SyscallResult> {
    let mut result = null_mut();
    unsafe {
        SyscallResult::create(raw_sys_map(
            vmo.map(|v| v.into_raw()).unwrap_or(0),
            flags.bits(),
            hint,
            length,
            &mut result,
        ))
        .map(|()| result)
    }
}

/// Unmap syscall
///
/// # Safety
///
/// The caller must ensure nothing still points to the region and that it is allowed to unmap it
#[inline]
pub unsafe fn sys_unmap(addr: vaddr_t, length: usize) -> SyscallResult {
    unsafe { SyscallResult::from_raw(raw_sys_unmap(addr, length)).unwrap() }
}

#[inline]
pub fn sys_read_args() -> Vec<u8> {
    unsafe {
        let size = raw_sys_read_args(null_mut(), 0);
        if size == 0 {
            return Vec::new();
        }

        let mut buffer: Vec<u8> = Vec::with_capacity(size);
        assert_eq!(
            raw_sys_read_args(buffer.as_mut_ptr(), buffer.capacity()),
            usize::MAX
        );
        buffer.set_len(size);
        buffer
    }
}

#[inline]
pub fn sys_read_args_string() -> String {
    String::from_utf8(sys_read_args()).unwrap()
}

#[inline]
pub fn sys_pid() -> Pid {
    unsafe { Pid::from_raw(raw_sys_pid()).unwrap() }
}

#[inline]
pub fn sys_log(level: u32, target: &str, message: &str) {
    unsafe {
        raw_sys_log(
            level,
            target.as_ptr(),
            target.len(),
            message.as_ptr(),
            message.len(),
        );
    }
}

// handle

/// Drops a handle
///
/// # Safety
///
/// The caller must ensure that nothing still references it as the kernel can reuse the id
#[inline]
pub unsafe fn sys_handle_drop(handle: Hid) -> SyscallResult {
    unsafe { SyscallResult::from_raw(raw_sys_handle_drop(handle.into_raw())).unwrap() }
}

#[inline]
pub fn sys_handle_clone(handle: Hid) -> Result<Hid, SyscallResult> {
    let mut new = 0;
    unsafe {
        SyscallResult::create(raw_sys_handle_clone(handle.into_raw(), &mut new))
            .map(|()| Hid::from_raw(new).unwrap())
    }
}

// object

#[inline]
pub fn sys_object_type(handle: Hid) -> Result<KernelObjectType, SyscallResult> {
    let mut val = 0;
    unsafe {
        SyscallResult::create(raw_sys_object_type(handle.into_raw(), &mut val))
            .map(|()| FromPrimitive::from_usize(val).unwrap())
    }
}

#[inline]
pub fn sys_object_wait(handle: Hid, on: ObjectSignal) -> Result<ObjectSignal, SyscallResult> {
    let mut res = 0;
    unsafe {
        SyscallResult::create(raw_sys_object_wait(handle.into_raw(), on.bits(), &mut res))
            .map(|()| ObjectSignal::from_bits_retain(res))
    }
}

#[inline]
pub fn sys_object_wait_port(handle: Hid, port: Hid, mask: ObjectSignal, key: u64) -> SyscallResult {
    unsafe {
        SyscallResult::from_raw(raw_sys_object_wait_port(
            handle.into_raw(),
            port.into_raw(),
            mask.bits(),
            key,
        ))
        .unwrap()
    }
}

// channel

#[inline]
pub fn sys_channel_create() -> (Hid, Hid) {
    unsafe {
        let mut left = 0;
        let mut right = 0;
        raw_sys_channel_create(&mut left, &mut right);
        (
            Hid::from_raw(left).unwrap_unchecked(),
            Hid::from_raw(right).unwrap_unchecked(),
        )
    }
}

#[inline]
pub fn sys_channel_read_vec<const N: usize>(
    handle: Hid,
    data: &mut Vec<u8>,
    resize: bool,
    blocking: bool,
) -> Result<heapless::Vec<Hid, N>, SyscallResult> {
    unsafe {
        data.clear();
        let mut handles: heapless::Vec<Hid, N> = heapless::Vec::new();

        loop {
            let mut data_len = data.capacity();
            let mut handles_len = handles.capacity();

            let res = SyscallResult::from_raw(raw_sys_channel_read(
                handle.into_raw(),
                data.as_mut_ptr(),
                &mut data_len,
                handles.as_mut_ptr().cast(),
                &mut handles_len,
            ))
            .unwrap();

            if resize && res == SyscallResult::ChannelBufferTooSmall {
                data.reserve(data_len);
                continue;
            }

            if blocking && res == SyscallResult::ChannelEmpty {
                sys_object_wait(handle, ObjectSignal::all()).unwrap();
                continue;
            }

            if res != SyscallResult::Ok {
                return Err(res);
            }

            data.set_len(data_len);
            handles.set_len(handles_len);

            return Ok(handles);
        }
    }
}

#[inline]
pub fn sys_channel_read_val<V: Sized, const N: usize>(
    handle: Hid,
    blocking: bool,
) -> Result<(V, heapless::Vec<Hid, N>), SyscallResult> {
    unsafe {
        let mut val: MaybeUninit<V> = MaybeUninit::uninit();
        let mut handles: heapless::Vec<Hid, N> = heapless::Vec::new();

        loop {
            let mut data_len = size_of::<V>();
            let mut handles_len = handles.capacity();
            let res = SyscallResult::from_raw(raw_sys_channel_read(
                handle.into_raw(),
                val.as_mut_ptr().cast(),
                &mut data_len,
                handles.as_mut_ptr().cast(),
                &mut handles_len,
            ))
            .unwrap();

            if blocking && res == SyscallResult::ChannelEmpty {
                sys_object_wait(
                    handle,
                    ObjectSignal::READABLE | ObjectSignal::CHANNEL_CLOSED,
                )
                .unwrap();
                continue;
            }

            if res != SyscallResult::Ok {
                return Err(res);
            }

            assert_eq!(data_len, size_of::<V>());

            handles.set_len(handles_len);

            return Ok((val.assume_init(), handles));
        }
    }
}

#[inline]
pub fn sys_channel_write(handle: Hid, data: &[u8], handles: &[Hid]) -> SyscallResult {
    unsafe {
        SyscallResult::from_raw(raw_sys_channel_write(
            handle.into_raw(),
            data.as_ptr(),
            data.len(),
            handles.as_ptr().cast(),
            handles.len(),
        ))
        .unwrap()
    }
}

#[inline]
pub fn sys_channel_write_val<V: Sized>(handle: Hid, val: &V, handles: &[Hid]) -> SyscallResult {
    unsafe {
        SyscallResult::from_raw(raw_sys_channel_write(
            handle.into_raw(),
            val as *const V as *const u8,
            size_of::<V>(),
            handles.as_ptr().cast(),
            handles.len(),
        ))
        .unwrap()
    }
}

// interrupt

#[inline]
pub fn sys_interrupt_create() -> Hid {
    unsafe { Hid::from_raw(raw_sys_interrupt_create()).unwrap() }
}

#[inline]
pub fn sys_interrupt_wait(handle: Hid) -> SyscallResult {
    unsafe { SyscallResult::from_raw(raw_sys_interrupt_wait(handle.into_raw())).unwrap() }
}

#[inline]
pub fn sys_interrupt_trigger(handle: Hid) -> SyscallResult {
    unsafe { SyscallResult::from_raw(raw_sys_interrupt_trigger(handle.into_raw())).unwrap() }
}

#[inline]
pub fn sys_interrupt_acknowledge(handle: Hid) -> SyscallResult {
    unsafe { SyscallResult::from_raw(raw_sys_interrupt_acknowledge(handle.into_raw())).unwrap() }
}

#[inline]
pub fn sys_interrupt_set_port(handle: Hid, port: Hid, key: u64) -> SyscallResult {
    unsafe {
        SyscallResult::from_raw(raw_sys_interrupt_set_port(
            handle.into_raw(),
            port.into_raw(),
            key,
        ))
        .unwrap()
    }
}

// port

#[inline]
pub fn sys_port_create() -> Hid {
    unsafe { Hid::from_raw(raw_sys_port_create()).unwrap() }
}

#[inline]
pub fn sys_port_wait(handle: Hid) -> Result<SysPortNotification, SyscallResult> {
    unsafe {
        let mut notif: MaybeUninit<sys_port_notification_t> = MaybeUninit::uninit();
        let res = raw_sys_port_wait(handle.into_raw(), notif.as_mut_ptr());

        SyscallResult::create(res)
            .map(|_| SysPortNotification::from_raw(notif.assume_init()).unwrap())
    }
}

#[inline]
pub fn sys_port_push(handle: Hid, notification: &SysPortNotification) -> SyscallResult {
    unsafe {
        let raw = notification.into_raw();
        SyscallResult::from_raw(raw_sys_port_push(handle.into_raw(), &raw)).unwrap()
    }
}

// process

/// Is used as a new threads entry point.
#[unsafe(naked)]
pub extern "C" fn sys_thread_bootstrapper(main: usize) {
    extern "C" fn _inner(main: usize) {
        // Recreate the function box that was passed from the syscall
        let func = unsafe { Box::from_raw(main as *mut Box<dyn FnOnce()>) };
        // We can release the outer box
        let func = Box::into_inner(func);
        // Call the function
        func.call_once(());

        // Function ended quit
        sys_exit()
    }
    // We can't hit start directly, as we need to maintain the 16 byte alignment of the ABI
    core::arch::naked_asm!(
        "call {}",
        sym _inner,
    )
}

#[inline]
pub fn sys_process_spawn_thread<F>(func: F) -> Tid
where
    F: FnOnce() + Send + Sync + 'static,
{
    unsafe {
        let boxed_func: Box<dyn FnOnce()> = Box::new(func);
        let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;
        let tid = raw_sys_process_spawn_thread(sys_thread_bootstrapper as *const (), raw.cast());
        Tid::from_raw(tid).unwrap()
    }
}

#[inline]
pub fn sys_process_exit_code(handle: Hid) -> Result<usize, SyscallResult> {
    unsafe {
        let mut exit = 0;
        let res = raw_sys_process_exit_code(handle.into_raw(), &mut exit);
        SyscallResult::create(res).map(|_| exit)
    }
}

// message

#[inline]
pub fn sys_message_create(data: &[u8]) -> Hid {
    unsafe { Hid::from_raw(raw_sys_message_create(data.as_ptr(), data.len())).unwrap() }
}

#[inline]
pub fn sys_message_size(handle: Hid) -> Result<usize, SyscallResult> {
    unsafe {
        let mut size = 0;
        SyscallResult::create(raw_sys_message_size(handle.into_raw(), &mut size)).map(|_| size)
    }
}

#[inline]
pub fn sys_message_read(handle: Hid, buf: &mut [u8]) -> SyscallResult {
    unsafe {
        SyscallResult::from_raw(raw_sys_message_read(
            handle.into_raw(),
            buf.as_mut_ptr(),
            buf.len(),
        ))
        .unwrap()
    }
}

// vmo

/// Creates a VMO mapping to the physical address `base` with a given length
///
/// # Safety
///
/// The caller must ensure it has rights to directly access the region and that it is a valid physical address range
#[inline]
pub unsafe fn sys_vmo_mmap_create(base: *mut (), length: usize) -> Hid {
    unsafe { Hid::from_raw(raw_sys_vmo_mmap_create(base, length)).unwrap() }
}

#[inline]
pub fn sys_vmo_anonymous_create(length: usize, flags: VMOAnonymousFlags) -> Hid {
    unsafe { Hid::from_raw(raw_sys_vmo_anonymous_create(length, flags.bits())).unwrap() }
}

#[inline]
pub fn sys_vmo_anonymous_pinned_addresses(
    handle: Hid,
    offset: usize,
    result: &mut [usize],
) -> SyscallResult {
    unsafe {
        SyscallResult::from_raw(raw_sys_vmo_anonymous_pinned_addresses(
            handle.into_raw(),
            offset,
            result.len(),
            result.as_mut_ptr(),
        ))
        .unwrap()
    }
}
