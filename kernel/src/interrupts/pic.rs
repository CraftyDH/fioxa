// PIC is not used in this kernel, we will only use APIC

use x86_64::{
    instructions::port::Port,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame},
};

pub unsafe fn disable_pic() {
    unsafe {
        // Supposedly takes long enough to write to 0x80 for init of hardware
        let mut wait_port: Port<u8> = Port::new(0x80);
        let mut wait = || wait_port.write(0);

        // Set ICW1
        let mut master_command: Port<u8> = Port::new(0x20);
        let mut master_data: Port<u8> = Port::new(0x21);
        let mut slave_command: Port<u8> = Port::new(0xA0);
        let mut slave_data: Port<u8> = Port::new(0xA1);

        // Start init seqence
        master_command.write(0x11);
        wait();
        slave_command.write(0x11);

        // We use 32 - 48 for spurius pic
        master_data.write(32);
        wait();
        slave_data.write(40);

        // ICW3: tell Master PIC that there is a slave PIC at IRQ2 (0000 0100)
        master_data.write(4);
        wait();
        // ICW3: tell Slave PIC its cascade identity (0000 0010)
        slave_data.write(2);
        wait();

        // ICW4: Set mode (8086)
        master_data.write(1);
        wait();
        slave_data.write(1);
        wait();

        // Mask everything
        master_data.write(0xFF);
        slave_data.write(0xFF);
    }
}

pub fn set_spurious_interrupts(idt: &mut InterruptDescriptorTable) {
    for i in 32..48 {
        idt[i].set_handler_fn(pic_spurious_interrupt);
    }
}

pub extern "x86-interrupt" fn pic_spurious_interrupt(_: InterruptStackFrame) {
    warn!("Interrupt received from PIC")
}
