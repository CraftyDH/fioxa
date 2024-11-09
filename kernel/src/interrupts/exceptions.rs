use x86_64::{
    registers::control::Cr2,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode},
    VirtAddr,
};

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    gdt::{DOUBLE_FAULT_IST_INDEX, PAGE_FAULT_IST_INDEX},
    scheduling::taskmanager::kill_bad_task,
    screen::gop::WRITER,
};

/// Generates a handler for each PIC lane.
/// Calls the appropiate handler in the HANDLERS list
#[macro_export]
macro_rules! exception_handler {
    ($handler: ident, $error:expr) => {
        pub extern "x86-interrupt" fn $handler(stack_frame: InterruptStackFrame) {
            // Find the relevent handler and call it
            warn!("EXCEPTION: caught {}, frame: {:?}", $error, stack_frame);
            kill_bad_task();
        }
    };
}

pub fn set_exceptions_idt(idt: &mut InterruptDescriptorTable) {
    exception_handler!(divide_error, "DIVIDE ERROR");
    idt.divide_error.set_handler_fn(divide_error);

    exception_handler!(debug, "DEBUG");
    idt.debug.set_handler_fn(debug);

    exception_handler!(nmi, "NON MASKABLE INTERRUPT");
    idt.non_maskable_interrupt.set_handler_fn(nmi);

    idt.breakpoint.set_handler_fn(breakpoint_handler);

    exception_handler!(overflow, "OVERFLOW");
    idt.overflow.set_handler_fn(overflow);

    exception_handler!(bound_range_exceeded, "BOUND RANGE EXCEEDED");
    idt.bound_range_exceeded
        .set_handler_fn(bound_range_exceeded);

    exception_handler!(invalid_opcode, "INVALID OPCODE");
    idt.invalid_opcode.set_handler_fn(invalid_opcode);

    exception_handler!(device_not_available, "DEVICE NOT AVAILABLE");
    idt.device_not_available
        .set_handler_fn(device_not_available);

    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(DOUBLE_FAULT_IST_INDEX);
    }

    idt.invalid_tss.set_handler_fn(invalid_tss);

    // idt.segment_not_present.set_handler_fn(segment_not_present);
    // idt.stack_segment_fault.set_handler_fn(handler)
    unsafe {
        idt.general_protection_fault
            .set_handler_fn(general_protection_handler);
        // .set_stack_index(tss::DOUBLE_FAULT_IST_INDEX);

        idt.page_fault
            .set_handler_addr(VirtAddr::new(page_fault_handler as u64))
            .set_stack_index(PAGE_FAULT_IST_INDEX);
        // .disable_interrupts(false);
    }
    // idt.alignment_check
    // idt.simd_floating_point
    // idt.virtualization
    // idt.security_exception
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    info!("BREAKPOINT {:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    WRITER.get().unwrap().lock().reset_screen(0xFF_00_00);

    panic!("EXCEPTION: DOUBLE FAULT {}\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn general_protection_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    error!(
        "EXCEPTION: GENERAL PROTECTION FAULT Error: {}\n{:#?}",
        error_code, stack_frame
    );
    kill_bad_task()
}

extern "x86-interrupt" fn invalid_tss(stack_frame: InterruptStackFrame, _error_code: u64) {
    panic!("EXCEPTION: INVALID TSS FAULT\n{:#?}", stack_frame);
}

// wrap_function_registers!(page_fault_handlers => page_fault_handler);

unsafe extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let context = CPULocalStorageRW::get_context();
    let addr = Cr2::read();

    // If context is zero very bad stuff happened
    if context == 0 {
        panic!(
            "EXCEPTION: PAGE FAULT: {:?}\n  Accessed Address: {:?} by {:?}",
            error_code, addr, stack_frame.instruction_pointer
        );
    }

    // We might deadlock on a lock that was held
    if CPULocalStorageRW::hold_interrupts_depth() > 0 {
        panic!(
            "EXCEPTION: PAGE FAULT (while held interrupts): {:?}\n  Accessed Address: {:?} by {:?}",
            error_code, addr, stack_frame.instruction_pointer
        );
    }

    // unsafe { WRITER.force_unlock() };
    // WRITER.lock().fill_screen(0xFF_00_00);
    // WRITER.lock().pos.y = 0;
    if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) {
        error!(
            "EXCEPTION: PAGE FAULT: Protection violation at {:?} {error_code:?}",
            addr
        );
        kill_bad_task()
    }

    let process = CPULocalStorageRW::get_current_task().process();
    let mut mem = process.memory.lock();
    if mem
        .page_mapper
        .page_fault_handler(addr.as_u64() as usize)
        .is_none()
    {
        warn!(
            "EXCEPTION: PAGE FAULT: Failed to map {:?} {:?}",
            addr, stack_frame.instruction_pointer
        );
        drop(mem);
        kill_bad_task()
    }
}
