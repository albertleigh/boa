//! Debug evaluation context
//!
//! This module provides a dedicated thread for JavaScript evaluation with the Context.
//! Similar to the actor model, this ensures Context never needs to be Send/Sync.

use crate::{Context, JsResult, Source, context::ContextBuilder};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;

/// Task to be executed in the evaluation thread
pub(super) enum EvalTask {
    /// Execute JavaScript code
    Execute {
        source: String,
        result_tx: mpsc::Sender<Result<String, String>>,
    },
    /// Get stack trace
    GetStackTrace {
        result_tx: mpsc::Sender<Result<Vec<StackFrameInfo>, String>>,
    },
    /// Evaluate expression in current frame
    Evaluate {
        expression: String,
        result_tx: mpsc::Sender<Result<String, String>>,
    },
    /// Terminate the evaluation thread
    Terminate,
}

/// Stack frame information
#[derive(Debug, Clone)]
pub(super) struct StackFrameInfo {
    pub function_name: String,
    pub source_path: String,
    pub line_number: u32,
    pub column_number: u32,
    pub pc: usize,
}

/// Debug evaluation context that runs in a dedicated thread
pub struct DebugEvalContext {
    task_tx: mpsc::Sender<EvalTask>,
    handle: Option<thread::JoinHandle<()>>,
}

/// Type for context setup function that can be sent across threads
type ContextSetup = Box<dyn FnOnce(&mut Context) -> JsResult<()> + Send>;

impl DebugEvalContext {
    /// Creates a new debug evaluation context
    /// Takes a setup function that will be called after Context is built in the eval thread
    pub fn new(
        context_setup: Box<dyn FnOnce(&mut Context) -> JsResult<()> + Send>,
        debugger: Arc<Mutex<crate::debugger::Debugger>>,
        condvar: Arc<Condvar>,
    ) -> JsResult<Self> {
        let (task_tx, task_rx) = mpsc::channel::<EvalTask>();

        let handle = thread::spawn(move || {
            // Set up debug hooks
            let hooks = std::rc::Rc::new(DebugHooks {
                debugger: debugger.clone(),
                condvar: condvar.clone(),
            });

            // Build the context with debug hooks IN THIS THREAD
            let mut context = match ContextBuilder::new().host_hooks(hooks).build() {
                Ok(ctx) => ctx,
                Err(e) => {
                    eprintln!("[DebugEvalContext] Failed to build context: {}", e);
                    return;
                }
            };

            // Call the setup function to register console and other runtimes
            if let Err(e) = context_setup(&mut context) {
                eprintln!("[DebugEvalContext] Context setup failed: {}", e);
                return;
            }

            // Attach the debugger to the context
            if let Err(e) = debugger.lock().unwrap().attach(&mut context) {
                eprintln!("[DebugEvalContext] Failed to attach debugger: {}", e);
                return;
            }

            eprintln!("[DebugEvalContext] Context created and debugger attached");

            // Process tasks
            while let Ok(task) = task_rx.recv() {
                match task {
                    EvalTask::Execute { source, result_tx } => {
                        let result = context.eval(Source::from_bytes(&source));
                        // Convert JsResult<JsValue> to Result<String, String> for sending
                        let send_result = match result {
                            Ok(v) => {
                                match v.to_string(&mut context) {
                                    Ok(js_str) => Ok(js_str.to_std_string_escaped()),
                                    Err(e) => Err(e.to_string()),
                                }
                            }
                            Err(e) => Err(e.to_string()),
                        };
                        let _ = result_tx.send(send_result);
                    }
                    EvalTask::GetStackTrace { result_tx } => {
                        let stack = crate::debugger::DebugApi::get_call_stack(&context);
                        let frames = stack
                            .iter()
                            .map(|frame| StackFrameInfo {
                                function_name: frame.function_name().to_std_string_escaped(),
                                source_path: frame.source_path().to_string(),
                                line_number: frame.line_number().unwrap_or(0),
                                column_number: frame.column_number().unwrap_or(0),
                                pc: frame.pc() as usize,
                            })
                            .collect();
                        let _ = result_tx.send(Ok(frames));
                    }
                    EvalTask::Evaluate { expression, result_tx } => {
                        // TODO: Implement proper frame evaluation
                        let _ = result_tx.send(Ok(format!("Evaluation not yet implemented: {}", expression)));
                    }
                    EvalTask::Terminate => {
                        eprintln!("[DebugEvalContext] Terminating evaluation thread");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            task_tx,
            handle: Some(handle),
        })
    }

    /// Executes JavaScript code in the evaluation thread
    pub fn execute(&self, source: String) -> Result<String, String> {
        let (result_tx, result_rx) = mpsc::channel();
        
        self.task_tx
            .send(EvalTask::Execute { source, result_tx })
            .map_err(|e| format!("Failed to send task: {}", e))?;
        
        result_rx
            .recv()
            .map_err(|e| format!("Failed to receive result: {}", e))?
    }

    /// Gets the current stack trace from the evaluation thread
    pub fn get_stack_trace(&self) -> Result<Vec<StackFrameInfo>, String> {
        let (result_tx, result_rx) = mpsc::channel();
        
        self.task_tx
            .send(EvalTask::GetStackTrace { result_tx })
            .map_err(|e| format!("Failed to send task: {}", e))?;
        
        result_rx
            .recv()
            .map_err(|e| format!("Failed to receive result: {}", e))?
    }

    /// Evaluates an expression in the current frame
    pub fn evaluate(&self, expression: String) -> Result<String, String> {
        let (result_tx, result_rx) = mpsc::channel();
        
        self.task_tx
            .send(EvalTask::Evaluate { expression, result_tx })
            .map_err(|e| format!("Failed to send task: {}", e))?;
        
        result_rx
            .recv()
            .map_err(|e| format!("Failed to receive result: {}", e))?
    }
}

impl Drop for DebugEvalContext {
    fn drop(&mut self) {
        // Send terminate signal
        let _ = self.task_tx.send(EvalTask::Terminate);
        
        // Wait for thread to finish
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Host hooks for the debug evaluation context
struct DebugHooks {
    debugger: Arc<Mutex<crate::debugger::Debugger>>,
    condvar: Arc<Condvar>,
}

impl crate::context::HostHooks for DebugHooks {
    fn on_debugger_statement(&self, context: &mut Context) -> JsResult<()> {
        let frame = crate::debugger::DebugApi::get_current_frame(context);
        eprintln!("[DebugHooks] Debugger statement hit at {}", frame);

        // Pause execution
        self.debugger.lock().unwrap().pause();

        // Wait for resume using condition variable
        self.wait_for_resume();

        Ok(())
    }

    fn on_step(&self, _context: &mut Context) -> JsResult<()> {
        if self.debugger.lock().unwrap().is_paused() {
            eprintln!("[DebugHooks] Paused - waiting for resume...");
            self.wait_for_resume();
        }

        Ok(())
    }
}

impl DebugHooks {
    fn wait_for_resume(&self) {
        let mut debugger_guard = self.debugger.lock().unwrap();

        while debugger_guard.is_paused() {
            debugger_guard = self.condvar.wait(debugger_guard).unwrap();
        }

        eprintln!("[DebugHooks] Resumed!");
    }
}
