use std::ops::ControlFlow;

use crate::{
    Context,
    vm::{CompletionRecord, opcode::Operation},
};

/// `Debugger` implements the Opcode Operation for `Opcode::Debugger`
///
/// Operation:
///  - Invokes the debugger hook from the host environment.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Debugger;

impl Debugger {
    #[inline(always)]
    pub(crate) fn operation((): (), context: &mut Context) -> ControlFlow<CompletionRecord> {
        // Call the debugger hook from the host hooks

        #[cfg(feature = "debugger")]
        match context.host_hooks().on_debugger_statement(context) {
            Ok(()) => ControlFlow::Continue(()),
            Err(err) => context.handle_error(err),
        }

        #[cfg(not(feature = "debugger"))]
        {
            ControlFlow::Continue(())
        }
    }
}

impl Operation for Debugger {
    const NAME: &'static str = "Debugger";
    const INSTRUCTION: &'static str = "INST - Debugger";
    const COST: u8 = 1;
}
