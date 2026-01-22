//! Debug evaluation context
//!
//! This module provides a dedicated thread for JavaScript evaluation with the Context.
//! Similar to the actor model, this ensures Context never needs to be Send/Sync.

use crate::{Context, JsResult, Source, context::ContextBuilder};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;

/// Event that can be sent from eval thread to DAP server
#[derive(Debug, Clone)]
pub enum DebugEvent {
    /// Execution stopped (paused)
    Stopped {
        reason: String,
        description: Option<String>,
    },
    /// Shutdown signal to terminate event forwarder thread
    Shutdown,
}

/// Task to be executed in the evaluation thread
pub(super) enum EvalTask {
    /// Execute JavaScript code (blocking - waits for result)
    Execute {
        source: String,
        result_tx: mpsc::Sender<Result<String, String>>,
    },
    /// Execute JavaScript code non-blocking (doesn't wait for result)
    /// Used for program execution that may hit breakpoints
    ExecuteNonBlocking {
        source: String,
        file_path: Option<String>,
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
    condvar: Arc<Condvar>,
    debugger: Arc<Mutex<crate::debugger::Debugger>>,
    /// Sender for debug events (kept to send shutdown signal)
    event_tx: mpsc::Sender<DebugEvent>,
}

/// Type for context setup function that can be sent across threads
type ContextSetup = Box<dyn FnOnce(&mut Context) -> JsResult<()> + Send>;

impl DebugEvalContext {
    /// Creates a new debug evaluation context
    /// Takes a setup function that will be called after Context is built in the eval thread
    /// Returns (DebugEvalContext, Receiver<DebugEvent>) - the receiver should be used to listen for events
    pub fn new(
        context_setup: Box<dyn FnOnce(&mut Context) -> JsResult<()> + Send>,
        debugger: Arc<Mutex<crate::debugger::Debugger>>,
        condvar: Arc<Condvar>,
    ) -> JsResult<(Self, mpsc::Receiver<DebugEvent>)> {
        let (task_tx, task_rx) = mpsc::channel::<EvalTask>();
        let (event_tx, event_rx) = mpsc::channel::<DebugEvent>();

        // Clone event_tx for the hooks, keep one for the struct
        let event_tx_for_hooks = event_tx.clone();

        // Clone Arc references for the thread
        let debugger_clone = debugger.clone();
        let condvar_clone = condvar.clone();

        // Wrap task_rx in Arc<Mutex> for sharing with hooks
        let task_rx = Arc::new(Mutex::new(task_rx));
        let task_rx_clone = task_rx.clone();

        let handle = thread::spawn(move || {
            // Set up debug hooks
            let hooks = std::rc::Rc::new(DebugHooks {
                debugger: debugger_clone.clone(),
                condvar: condvar_clone.clone(),
                event_tx: event_tx_for_hooks,
                task_rx: task_rx_clone,
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
            if let Err(e) = debugger_clone.lock().unwrap().attach(&mut context) {
                eprintln!("[DebugEvalContext] Failed to attach debugger: {}", e);
                return;
            }

            eprintln!("[DebugEvalContext] Context created and debugger attached");

            // Process tasks
            loop {
                let task = match task_rx.lock().unwrap().recv() {
                    Ok(t) => t,
                    Err(_) => break,
                };
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
                    EvalTask::ExecuteNonBlocking { source, file_path } => {
                        eprintln!("[DebugEvalContext] Starting non-blocking execution{}",
                            file_path.as_ref().map(|p| format!(" of {}", p)).unwrap_or_default());

                        // Execute without blocking - the eval thread continues to process other tasks
                        let result = context.eval(Source::from_bytes(&source));

                        match result {
                            Ok(v) => {
                                if !v.is_undefined() {
                                    eprintln!("[DebugEvalContext] Execution completed with result: {}",
                                        v.display());
                                } else {
                                    eprintln!("[DebugEvalContext] Execution completed");
                                }
                            }
                            Err(e) => {
                                eprintln!("[DebugEvalContext] Execution error: {}", e);
                            }
                        }

                        // Run any pending jobs (promises, etc.)
                        if let Err(e) = context.run_jobs() {
                            eprintln!("[DebugEvalContext] Job execution error: {}", e);
                        }
                    }
                    EvalTask::Terminate => {
                        eprintln!("[DebugEvalContext] Terminating evaluation thread");
                        break;
                    }
                    // Handle inspection tasks using common helper
                    other => {
                        DebugHooks::process_inspection_task(other, &mut context);
                    }
                }
            } // End task processing loop
        });

        let ctx = Self {
            task_tx,
            handle: Some(handle),
            condvar: condvar.clone(),
            debugger: debugger.clone(),
            event_tx,
        };
        
        Ok((ctx, event_rx))
    }

    /// Executes JavaScript code in the evaluation thread (blocking)
    /// This will wait for the result, so it should NOT be used for program execution
    /// that may hit breakpoints. Use execute_async instead.
    pub fn execute(&self, source: String) -> Result<String, String> {
        let (result_tx, result_rx) = mpsc::channel();
        
        self.task_tx
            .send(EvalTask::Execute { source, result_tx })
            .map_err(|e| format!("Failed to send task: {}", e))?;

        // This will block the current thread until the result is received
        result_rx
            .recv()
            .map_err(|e| format!("Failed to receive result: {}", e))?
    }

    /// Executes JavaScript code asynchronously without blocking
    /// The execution happens in the eval thread and this method returns immediately
    /// Use this for program execution that may hit breakpoints
    pub fn execute_async(&self, source: String, file_path: Option<String>) -> Result<(), String> {
        self.task_tx
            .send(EvalTask::ExecuteNonBlocking { source, file_path })
            .map_err(|e| format!("Failed to send task: {}", e))?;
        
        Ok(())
    }

    /// Gets the current stack trace from the evaluation thread
    pub fn get_stack_trace(&self) -> Result<Vec<StackFrameInfo>, String> {
        let (result_tx, result_rx) = mpsc::channel();

        self.task_tx
            .send(EvalTask::GetStackTrace { result_tx })
            .map_err(|e| format!("Failed to send task: {}", e))?;

        // Notify condvar ONLY if debugger is paused
        // This wakes wait_for_resume to process the task immediately
        if self.debugger.lock().unwrap().is_paused() {
            self.condvar.notify_all();
        }
        
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
        
        // Notify condvar ONLY if debugger is paused
        // This wakes wait_for_resume to process the task immediately
        if self.debugger.lock().unwrap().is_paused() {
            self.condvar.notify_all();
        }
        
        result_rx
            .recv()
            .map_err(|e| format!("Failed to receive result: {}", e))?
    }
}

impl Drop for DebugEvalContext {
    fn drop(&mut self) {
        eprintln!("[DebugEvalContext] Dropping - initiating shutdown");

        // Signal shutdown to break any wait_for_resume loops
        {
            let mut debugger = self.debugger.lock().unwrap();
            debugger.shutdown();
        }
        
        // Wake up any threads waiting on the condvar
        self.condvar.notify_all();
        
        // Send shutdown event to terminate any event forwarder threads
        let _ = self.event_tx.send(DebugEvent::Shutdown);
        
        // Send terminate signal to eval thread
        let _ = self.task_tx.send(EvalTask::Terminate);

        // Wait for thread to finish with a timeout
        if let Some(handle) = self.handle.take() {
            match handle.join() {
                Ok(_) => eprintln!("[DebugEvalContext] Thread joined successfully"),
                Err(e) => eprintln!("[DebugEvalContext] Thread join failed: {:?}", e),
            }
        }
    }
}

/// Host hooks for the debug evaluation context
struct DebugHooks {
    debugger: Arc<Mutex<crate::debugger::Debugger>>,
    condvar: Arc<Condvar>,
    event_tx: mpsc::Sender<DebugEvent>,
    task_rx: Arc<Mutex<mpsc::Receiver<EvalTask>>>,
}

impl crate::context::HostHooks for DebugHooks {
    fn on_debugger_statement(&self, context: &mut Context) -> JsResult<()> {
        let frame = crate::debugger::DebugApi::get_current_frame(context);
        eprintln!("[DebugHooks] Debugger statement hit at {}", frame);

        // Pause execution
        self.debugger.lock().unwrap().pause();

        // Send stopped event to DAP server
        let _ = self.event_tx.send(DebugEvent::Stopped {
            reason: "pause".to_string(),
            description: Some(format!("Paused on debugger statement at {}", frame)),
        });

        // Wait for resume using condition variable
        // Returns error if shutting down
        // Passes context to allow processing inspection tasks while paused
        self.wait_for_resume(context)?;

        Ok(())
    }

    fn on_step(&self, context: &mut Context) -> JsResult<()> {
        if self.debugger.lock().unwrap().is_paused() {
            eprintln!("[DebugHooks] Paused - waiting for resume...");
            
            // Send stopped event to DAP server
            let _ = self.event_tx.send(DebugEvent::Stopped {
                reason: "step".to_string(),
                description: Some("Paused on step".to_string()),
            });
            
            // Returns error if shutting down
            // Passes context to allow processing inspection tasks while paused
            self.wait_for_resume(context)?;
        }

        Ok(())
    }
}

impl DebugHooks {
    /// Process inspection tasks (GetStackTrace, Evaluate) that can run while paused
    /// Returns true if task was processed, false if it should be skipped
    fn process_inspection_task(task: EvalTask, context: &mut Context) -> bool {
        match task {
            EvalTask::GetStackTrace { result_tx } => {
                let stack = crate::debugger::DebugApi::get_call_stack(context);
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
                true
            }
            EvalTask::Evaluate { expression, result_tx } => {
                // TODO: Implement proper frame evaluation
                let _ = result_tx.send(Ok(format!("Evaluation not yet implemented: {}", expression)));
                true
            }
            _ => false, // Not an inspection task
        }
    }

    /// Wait for resume while continuing to process inspection tasks
    /// This prevents deadlock when DAP client requests stackTrace/evaluate while paused
    fn wait_for_resume(&self, context: &mut Context) -> JsResult<()> {
        eprintln!("[DebugHooks] Entering wait_for_resume - will process tasks while waiting");
        
        loop {
            // Process any pending inspection tasks before waiting
            // Do this WITHOUT holding debugger lock to avoid contention
            loop {
                match self.task_rx.lock().unwrap().try_recv() {
                    Ok(task) => {
                        // Handle non-inspection tasks specially
                        match task {
                            EvalTask::Execute { .. } | EvalTask::ExecuteNonBlocking { .. } => {
                                eprintln!("[DebugHooks] Dropping execution task received while paused");
                            }
                            EvalTask::Terminate => {
                                eprintln!("[DebugHooks] Terminate signal received while paused");
                                return Err(crate::JsNativeError::error()
                                    .with_message("Eval thread terminating")
                                    .into());
                            }
                            // Process inspection tasks
                            other => {
                                if Self::process_inspection_task(other, context) {
                                    eprintln!("[DebugHooks] Processed inspection task while paused");
                                }
                            }
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        // No more pending tasks - exit drain loop
                        break;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        eprintln!("[DebugHooks] Task channel disconnected");
                        return Err(crate::JsNativeError::error()
                            .with_message("Task channel closed")
                            .into());
                    }
                }
            }
            
            // NOW lock debugger once to check state and wait
            // This is the ONLY lock acquisition per loop iteration
            let mut debugger_guard = self.debugger.lock().unwrap();
            
            // Check if we should exit
            if !debugger_guard.is_paused() {
                eprintln!("[DebugHooks] Resumed!");
                return Ok(());
            }
            if debugger_guard.is_shutting_down() {
                eprintln!("[DebugHooks] Shutting down - aborting execution");
                return Err(crate::JsNativeError::error()
                    .with_message("Debugger shutting down")
                    .into());
            }
            
            // Still paused - wait on condvar (keeps debugger_guard locked)
            // Will be woken by:
            // 1. resume() - to continue execution
            // 2. notify_all() from get_stack_trace/evaluate - to process inspection tasks
            // 3. shutdown() - to terminate cleanly
            eprintln!("[DebugHooks] Waiting on condvar...");
            debugger_guard = self.condvar.wait(debugger_guard).unwrap();
            eprintln!("[DebugHooks] Condvar woken - checking for tasks and state");
            
            // debugger_guard dropped here, releasing lock
            // Loop back to: drain tasks (unlocked) → check state (locked) → wait
        }
    }
}
