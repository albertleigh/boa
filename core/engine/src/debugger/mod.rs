//! Boa's JavaScript Debugger API
//!
//! This module provides a comprehensive debugging interface for JavaScript code
//! running in the Boa engine, inspired by SpiderMonkey's debugger architecture.
//!
//! # Overview
//!
//! The debugger API consists of several key components:
//!
//! - [`Debugger`]: The main debugger interface that can be attached to a context
//! - [`DebugApi`]: Static API for debugger operations and event notifications
//! - Reflection objects: Safe wrappers for inspecting debuggee state
//!   - [`DebuggerFrame`]: Represents a call frame
//!   - [`DebuggerScript`]: Represents a compiled script/function
//!   - [`DebuggerObject`]: Represents an object in the debuggee
//!
//! # Architecture
//!
//! The debugger uses an event-based hook system similar to SpiderMonkey:
//!
//! - `on_debugger_statement`: Called when `debugger;` statement is executed
//! - `on_enter_frame`: Called when entering a new call frame
//! - `on_exit_frame`: Called when exiting a call frame
//! - `on_exception_unwind`: Called when an exception is being unwound
//! - `on_new_script`: Called when a new script/function is compiled
//!
//! # Example
//!
//! ```rust,ignore
//! use boa_engine::{Context, debugger::Debugger};
//!
//! let mut context = Context::default();
//! let mut debugger = Debugger::new();
//!
//! // Attach the debugger to the context
//! debugger.attach(&mut context);
//!
//! // Set a breakpoint
//! debugger.set_breakpoint("script.js", 10);
//!
//! // Execute code - debugger will pause at breakpoints
//! context.eval(Source::from_bytes("debugger; console.log('test');"));
//! ```

pub mod api;
pub mod breakpoint;
pub mod dap;
pub mod hooks;
pub mod reflection;
pub mod state;

pub use api::DebugApi;
pub use breakpoint::{Breakpoint, BreakpointId, BreakpointSite};
pub use hooks::{DebuggerEventHandler, DebuggerHooks};
pub use reflection::{DebuggerFrame, DebuggerObject, DebuggerScript};
pub use state::{Debugger, DebuggerState, StepMode};

use crate::JsResult;

/// Result type for debugger operations.
pub type DebugResult<T> = JsResult<T>;

/// Unique identifier for a script or code block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptId(pub(crate) usize);

/// Unique identifier for a call frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId(pub(crate) usize);
