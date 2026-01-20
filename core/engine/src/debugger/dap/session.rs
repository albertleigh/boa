//! Debug session management
//!
//! This module implements the debug session that connects the DAP protocol
//! with Boa's debugger API.

use super::messages::*;
use crate::{
    Context, JsResult,
    debugger::{BreakpointId, DebugApi, Debugger, ScriptId},
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A debug session manages the connection between DAP and Boa's debugger
pub struct DebugSession {
    /// The Boa debugger instance
    debugger: Arc<Mutex<Debugger>>,

    /// Mapping from source paths to script IDs
    source_to_script: HashMap<String, ScriptId>,

    /// Mapping from DAP breakpoint IDs to Boa breakpoint IDs
    breakpoint_mapping: HashMap<i64, BreakpointId>,

    /// Next DAP breakpoint ID
    next_breakpoint_id: i64,

    /// Whether the session is initialized
    initialized: bool,

    /// Whether the session is running
    running: bool,

    /// Current thread ID (Boa is single-threaded, so this is always 1)
    thread_id: i64,

    /// Stopped reason
    stopped_reason: Option<String>,

    /// Variable references for scopes and objects
    variable_references: HashMap<i64, VariableReference>,
    next_variable_reference: i64,
}

#[derive(Debug, Clone)]
enum VariableReference {
    Scope {
        frame_id: i64,
        scope_type: ScopeType,
    },
    Object {
        object_id: String,
    },
}

#[derive(Debug, Clone, Copy)]
enum ScopeType {
    Local,
    Global,
    Closure,
}

impl DebugSession {
    /// Creates a new debug session
    pub fn new(debugger: Arc<Mutex<Debugger>>) -> Self {
        Self {
            debugger,
            source_to_script: HashMap::new(),
            breakpoint_mapping: HashMap::new(),
            next_breakpoint_id: 1,
            initialized: false,
            running: false,
            thread_id: 1,
            stopped_reason: None,
            variable_references: HashMap::new(),
            next_variable_reference: 1,
        }
    }

    /// Handles the initialize request
    pub fn handle_initialize(
        &mut self,
        _args: InitializeRequestArguments,
    ) -> JsResult<Capabilities> {
        self.initialized = true;

        Ok(Capabilities {
            supports_configuration_done_request: true,
            supports_function_breakpoints: false,
            supports_conditional_breakpoints: true,
            supports_hit_conditional_breakpoints: true,
            supports_evaluate_for_hovers: true,
            supports_step_back: false,
            supports_set_variable: false,
            supports_restart_frame: false,
            supports_goto_targets_request: false,
            supports_step_in_targets_request: false,
            supports_completions_request: false,
            supports_modules_request: false,
            supports_restart_request: false,
            supports_exception_options: false,
            supports_value_formatting_options: true,
            supports_exception_info_request: false,
            supports_terminate_debuggee: true,
            supports_delayed_stack_trace_loading: false,
            supports_loaded_sources_request: false,
            supports_log_points: true,
            supports_terminate_threads_request: false,
            supports_set_expression: false,
            supports_terminate_request: true,
            supports_data_breakpoints: false,
            supports_read_memory_request: false,
            supports_disassemble_request: false,
            supports_cancel_request: false,
            supports_breakpoint_locations_request: false,
            supports_clipboard_context: false,
        })
    }

    /// Handles the launch request
    /// The actual execution is handled by the CLI, this just stores the launch state
    pub fn handle_launch(&mut self, _args: LaunchRequestArguments) -> JsResult<()> {
        // The CLI will handle creating the context, setting up runtime,
        // executing the program, and capturing output
        self.running = false;
        Ok(())
    }

    /// Handles the attach request
    pub fn handle_attach(&mut self, _args: AttachRequestArguments) -> JsResult<()> {
        // Attach will be handled by the CLI tool
        Ok(())
    }

    /// Handles setting breakpoints
    pub fn handle_set_breakpoints(
        &mut self,
        args: SetBreakpointsArguments,
    ) -> JsResult<SetBreakpointsResponseBody> {
        let mut breakpoints = Vec::new();

        // Get the source path
        let source_path = args.source.path.clone().unwrap_or_else(|| {
            args.source
                .name
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        });

        // Get the script ID for this source
        // For now, we'll use a placeholder since we need line-to-PC mapping
        let script_id = *self
            .source_to_script
            .entry(source_path.clone())
            .or_insert(ScriptId(0));

        if let Some(source_breakpoints) = args.breakpoints {
            for bp in source_breakpoints {
                // TODO: Map line number to PC offset
                // For now, we'll create a placeholder
                let boa_bp_id = {
                    let mut debugger = self.debugger.lock().unwrap();
                    debugger.set_breakpoint(script_id, bp.line as u32)
                };

                let dap_bp_id = self.next_breakpoint_id;
                self.next_breakpoint_id += 1;

                self.breakpoint_mapping.insert(dap_bp_id, boa_bp_id);

                breakpoints.push(Breakpoint {
                    id: Some(dap_bp_id),
                    verified: true,
                    message: None,
                    source: Some(args.source.clone()),
                    line: Some(bp.line),
                    column: bp.column,
                    end_line: None,
                    end_column: None,
                });
            }
        }

        Ok(SetBreakpointsResponseBody { breakpoints })
    }

    /// Handles the continue request
    pub fn handle_continue(&mut self, _args: ContinueArguments) -> JsResult<ContinueResponseBody> {
        self.debugger.lock().unwrap().resume();
        self.running = true;
        self.stopped_reason = None;

        Ok(ContinueResponseBody {
            all_threads_continued: true,
        })
    }

    /// Handles the next (step over) request
    pub fn handle_next(&mut self, _args: NextArguments, frame_depth: usize) -> JsResult<()> {
        self.debugger.lock().unwrap().step_over(frame_depth);
        self.running = true;
        self.stopped_reason = None;
        Ok(())
    }

    /// Handles the step in request
    pub fn handle_step_in(&mut self, _args: StepInArguments) -> JsResult<()> {
        self.debugger.lock().unwrap().step_in();
        self.running = true;
        self.stopped_reason = None;
        Ok(())
    }

    /// Handles the step out request
    pub fn handle_step_out(&mut self, _args: StepOutArguments, frame_depth: usize) -> JsResult<()> {
        self.debugger.lock().unwrap().step_out(frame_depth);
        self.running = true;
        self.stopped_reason = None;
        Ok(())
    }

    /// Handles the stack trace request
    pub fn handle_stack_trace(
        &mut self,
        _args: StackTraceArguments,
        context: &Context,
    ) -> JsResult<StackTraceResponseBody> {
        let stack = DebugApi::get_call_stack(context);

        let stack_frames: Vec<StackFrame> = stack
            .iter()
            .enumerate()
            .map(|(i, frame)| {
                let source = Source {
                    name: Some(frame.function_name().to_std_string_escaped()),
                    path: Some(frame.source_path().to_string()),
                    source_reference: None,
                    presentation_hint: None,
                    origin: None,
                    sources: None,
                    adapter_data: None,
                    checksums: None,
                };

                StackFrame {
                    id: i as i64,
                    name: frame.function_name().to_std_string_escaped(),
                    source: Some(source),
                    line: frame.line_number().unwrap_or(0) as i64,
                    column: frame.column_number().unwrap_or(0) as i64,
                    end_line: None,
                    end_column: None,
                    can_restart: false,
                    instruction_pointer_reference: Some(format!("{}", frame.pc())),
                    module_id: None,
                    presentation_hint: None,
                }
            })
            .collect();

        Ok(StackTraceResponseBody {
            stack_frames,
            total_frames: Some(stack.len() as i64),
        })
    }

    /// Handles the scopes request
    pub fn handle_scopes(&mut self, args: ScopesArguments) -> JsResult<ScopesResponseBody> {
        // Create variable references for different scopes
        let local_ref = self.next_variable_reference;
        self.next_variable_reference += 1;
        self.variable_references.insert(
            local_ref,
            VariableReference::Scope {
                frame_id: args.frame_id,
                scope_type: ScopeType::Local,
            },
        );

        let global_ref = self.next_variable_reference;
        self.next_variable_reference += 1;
        self.variable_references.insert(
            global_ref,
            VariableReference::Scope {
                frame_id: args.frame_id,
                scope_type: ScopeType::Global,
            },
        );

        let scopes = vec![
            Scope {
                name: "Local".to_string(),
                presentation_hint: Some("locals".to_string()),
                variables_reference: local_ref,
                named_variables: None,
                indexed_variables: None,
                expensive: false,
                source: None,
                line: None,
                column: None,
                end_line: None,
                end_column: None,
            },
            Scope {
                name: "Global".to_string(),
                presentation_hint: Some("globals".to_string()),
                variables_reference: global_ref,
                named_variables: None,
                indexed_variables: None,
                expensive: false,
                source: None,
                line: None,
                column: None,
                end_line: None,
                end_column: None,
            },
        ];

        Ok(ScopesResponseBody { scopes })
    }

    /// Handles the variables request
    pub fn handle_variables(
        &mut self,
        _args: VariablesArguments,
    ) -> JsResult<VariablesResponseBody> {
        // TODO: Implement variable inspection using DebuggerFrame::eval()
        // For now, return empty list
        Ok(VariablesResponseBody { variables: vec![] })
    }

    /// Handles the evaluate request
    pub fn handle_evaluate(&mut self, args: EvaluateArguments) -> JsResult<EvaluateResponseBody> {
        // TODO: Implement expression evaluation using DebuggerFrame::eval()
        Ok(EvaluateResponseBody {
            result: format!("Evaluation not yet implemented: {}", args.expression),
            type_: Some("string".to_string()),
            presentation_hint: None,
            variables_reference: 0,
            named_variables: None,
            indexed_variables: None,
        })
    }

    /// Handles the threads request
    pub fn handle_threads(&mut self) -> JsResult<ThreadsResponseBody> {
        Ok(ThreadsResponseBody {
            threads: vec![Thread {
                id: self.thread_id,
                name: "Main Thread".to_string(),
            }],
        })
    }

    /// Notifies the session that execution has stopped
    pub fn notify_stopped(&mut self, reason: String) {
        self.running = false;
        self.stopped_reason = Some(reason);
    }

    /// Gets the current thread ID
    pub fn thread_id(&self) -> i64 {
        self.thread_id
    }

    /// Checks if the session is running
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Gets the stopped reason
    pub fn stopped_reason(&self) -> Option<&str> {
        self.stopped_reason.as_deref()
    }
}
