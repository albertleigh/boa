//! HostHooks implementation for debugger integration
//!
//! This module provides a HostHooks implementation that integrates the debugger
//! with the VM execution loop.

use super::{Debugger, DebuggerState};
use crate::{Context, JsResult, context::HostHooks};
use std::sync::{Arc, Mutex};

/// HostHooks implementation that integrates debugger functionality
///
/// This wraps a `Debugger` instance and implements the `HostHooks` trait
/// to provide breakpoint checking, pause/resume, and stepping support.
///
/// # Example
///
/// ```rust,ignore
/// use boa_engine::{Context, debugger::{Debugger, DebuggerHostHooks}};
/// use std::sync::{Arc, Mutex};
///
/// let debugger = Arc::new(Mutex::new(Debugger::new()));
/// let hooks = DebuggerHostHooks::new(debugger.clone());
///
/// let mut context = Context::builder()
///     .host_hooks(hooks)
///     .build()
///     .unwrap();
/// ```
pub struct DebuggerHostHooks {
    debugger: Arc<Mutex<Debugger>>,
}

impl DebuggerHostHooks {
    /// Creates a new DebuggerHostHooks with the given debugger
    pub fn new(debugger: Arc<Mutex<Debugger>>) -> Self {
        Self { debugger }
    }

    /// Gets a reference to the debugger
    pub fn debugger(&self) -> Arc<Mutex<Debugger>> {
        self.debugger.clone()
    }
}

impl HostHooks for DebuggerHostHooks {
    fn on_step(&self, context: &mut Context) -> JsResult<()> {
        let mut debugger = self.debugger.lock().unwrap();

        // Skip if debugger is not attached
        if !debugger.is_attached() {
            return Ok(());
        }

        // Get current frame and PC
        let frame = context.vm.frame();

        let pc = frame.pc;
        let frame_depth = context.vm.frames.len();

        // Check if we should pause for stepping
        if debugger.should_pause_for_step(frame_depth) {
            debugger.pause();
        }

        // Check for breakpoint at current location
        if let Some(script_id) = frame.code_block.script_id() {
            if debugger.has_breakpoint(script_id, pc) {
                // Breakpoint hit! Pause execution
                debugger.pause();

                // If debugger has hooks, call the on_breakpoint hook
                // (This is optional - for now we just pause)
                // TODO: Call on_breakpoint hook on the debugger's internal hooks
            }
        }

        // If paused, wait here (this is a simple spin-wait for now)
        // In a real implementation, this should use proper wait mechanisms
        while debugger.is_paused() {
            drop(debugger); // Release lock while waiting
            std::thread::sleep(std::time::Duration::from_millis(10));
            debugger = self.debugger.lock().unwrap();
        }

        Ok(())
    }

    fn on_debugger_statement(&self, context: &mut Context) -> JsResult<()> {
        let mut debugger = self.debugger.lock().unwrap();

        if !debugger.is_attached() {
            return Ok(());
        }

        // Pause on debugger statement
        debugger.pause();

        Ok(())
    }

    fn on_exception_unwind(&self, context: &mut Context) -> JsResult<bool> {
        let debugger = self.debugger.lock().unwrap();

        if !debugger.is_attached() {
            return Ok(false);
        }

        // Check if we should pause on exceptions
        // For now, only pause if already in stepping mode
        match debugger.state() {
            DebuggerState::Stepping(_) => Ok(true),
            _ => Ok(false),
        }
    }
}
