use x86_64::{
    registers::control::Cr2,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode},
};

use crate::{gdt::tss, screen::gop::WRITER};

/// Generates a handler for each PIC lane.
/// Calls the appropiate handler in the HANDLERS list
#[macro_export]
macro_rules! exception_handler {
    ($handler: ident, $error:expr) => {
        extern "x86-interrupt" fn $handler(stack_frame: InterruptStackFrame) {
            // Find the relevent handler and call it

            panic!("EXCEPTION: caught {}, frame: {:?}", $error, stack_frame)
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
            .set_stack_index(tss::DOUBLE_FAULT_IST_INDEX);
    }

    idt.invalid_tss.set_handler_fn(invalid_tss);

    // idt.segment_not_present.set_handler_fn(segment_not_present);
    // idt.stack_segment_fault.set_handler_fn(handler)

    idt.general_protection_fault
        .set_handler_fn(general_protection_handler);

    idt.page_fault.set_handler_fn(page_fault_handler);

    // idt.alignment_check
    // idt.simd_floating_point
    // idt.virtualization
    // idt.security_exception
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("BREAKPOINT {:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    WRITER.lock().fill_screen(0xFF_00_00);
    WRITER.lock().pos.y = 0;

    panic!("EXCEPTION: DOUBLE FAULT {}\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn general_protection_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!(
        "EXCEPTION: GENERAL PROTECTION FAULT Error: {}\n{:#?}",
        error_code, stack_frame
    );
}

extern "x86-interrupt" fn invalid_tss(stack_frame: InterruptStackFrame, _error_code: u64) {
    panic!("EXCEPTION: INVALID TSS FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    // unsafe { WRITER.force_unlock() };
    WRITER.lock().fill_screen(0xFF_00_00);
    WRITER.lock().pos.y = 0;
    println!("EXCEPTION: PAGE FAULT");
    println!("Accessed Address: {:?}", Cr2::read());
    println!("{:#?}", stack_frame);
    println!("Error Code: {:?}", error_code);

    loop {}
}

// #[test_case]
// fn test_breakpoint_exception() {
//     // test a break point
//     // Execution should continue therefore we can test this here.
//     x86_64::instructions::interrupts::int3();
// }
