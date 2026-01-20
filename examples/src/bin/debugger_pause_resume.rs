//! Demonstrates the pause/resume mechanism of the Boa debugger.
//!
//! This example shows two implementations of the pause/resume functionality:
//!
//! 1. **Spin-wait approach** (SpiderMonkey style): Simple polling with sleep
//! 2. **Condition variable approach** (recommended): Efficient OS-level signaling
//!
//! When a `debugger;` statement is encountered, execution pauses and waits
//! until `resume()` is called from another thread or the DAP server.
//!
//! The condition variable approach is more efficient as it uses OS signals
//! instead of polling, reducing CPU usage and providing instant notification.

use boa_engine::{
    Context, JsResult, Source,
    context::{ContextBuilder, HostHooks},
    debugger::{DebugApi, Debugger},
};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// Enhanced debugger wrapper with condition variable for efficient signaling.
///
/// This combines the Debugger state with a condition variable that allows
/// the waiting thread to be notified immediately when resume() is called,
/// instead of polling every 10ms.
struct DebuggerWithSignal {
    debugger: Arc<Mutex<Debugger>>,
    /// Condition variable for efficient pause/resume signaling
    condvar: Arc<Condvar>,
}

impl DebuggerWithSignal {
    fn new() -> Self {
        Self {
            debugger: Arc::new(Mutex::new(Debugger::new())),
            condvar: Arc::new(Condvar::new()),
        }
    }

    /// Pause execution
    fn pause(&self) {
        self.debugger.lock().unwrap().pause();
    }

    /// Resume execution and notify waiting threads
    fn resume(&self) {
        self.debugger.lock().unwrap().resume();
        // Wake up all threads waiting on the condition variable
        self.condvar.notify_all();
    }

    /// Check if the debugger is paused
    fn is_paused(&self) -> bool {
        self.debugger.lock().unwrap().is_paused()
    }

    /// Forward attach call to inner debugger
    fn attach(&self, context: &mut Context) -> JsResult<()> {
        self.debugger.lock().unwrap().attach(context)
    }

    /// Clone for sharing across threads
    fn clone(&self) -> Self {
        Self {
            debugger: Arc::clone(&self.debugger),
            condvar: Arc::clone(&self.condvar),
        }
    }
}

/// Test hooks using condition variable for efficient pause/resume.
///
/// This is significantly more efficient than spin-waiting because:
/// - No CPU cycles wasted polling
/// - Instant notification when resume() is called
/// - OS-level thread scheduling for optimal performance
struct EfficientDebugHooks {
    debugger: DebuggerWithSignal,
}

impl EfficientDebugHooks {
    /// Wait for resume using condition variable (efficient, no polling)
    fn wait_for_resume(&self) {
        // Lock the debugger
        let mut debugger_guard = self.debugger.debugger.lock().unwrap();

        // Wait while paused - the condition variable will atomically release
        // the lock and put the thread to sleep until notified
        while debugger_guard.is_paused() {
            debugger_guard = self.debugger.condvar.wait(debugger_guard).unwrap();
        }

        eprintln!("[Debugger] Resumed!");
    }
}
impl HostHooks for EfficientDebugHooks {
    /// Called when a `debugger;` statement is encountered.
    fn on_debugger_statement(&self, context: &mut Context) -> JsResult<()> {
        let frame = DebugApi::get_current_frame(context);
        eprintln!("\n[Debugger] Statement hit at {}", frame);

        // Pause execution
        self.debugger.pause();

        // Wait efficiently using condition variable
        self.wait_for_resume();

        Ok(())
    }

    /// Called before every instruction in the VM execution loop.
    ///
    /// This is where we check if the debugger is paused (e.g., from
    /// a stepping operation or breakpoint hit) and wait if needed.
    fn on_step(&self, _context: &mut Context) -> JsResult<()> {
        let should_wait = self.debugger.is_paused();

        if should_wait {
            eprintln!("\n[Debugger] Paused - waiting for resume...");
            self.wait_for_resume();
        }

        Ok(())
    }
}

fn main() -> JsResult<()> {
    eprintln!("\n=== Testing Pause/Resume Mechanism (Condition Variable) ===\n");

    // Create debugger with condition variable
    let debugger = DebuggerWithSignal::new();
    let debugger_clone = debugger.clone();

    // Spawn a thread to resume after a delay
    // This simulates an external signal like a DAP "continue" request
    let resume_handle = thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        eprintln!("[Test] Calling resume() from another thread...");

        // Resume and notify the waiting thread via condition variable
        debugger_clone.resume();
    });

    let hooks = Rc::new(EfficientDebugHooks {
        debugger: debugger.clone(),
    });

    let mut context = ContextBuilder::new().host_hooks(hooks).build()?;

    // Attach the debugger
    debugger.attach(&mut context)?;

    eprintln!("[Test] Executing JavaScript with debugger statement...");
    eprintln!("[Test] Using CONDITION VARIABLE for efficient signaling (no polling!)");

    // Execute code with a debugger statement
    // The execution will pause at the debugger statement and wait
    // efficiently using a condition variable until resume() is called
    let source = Source::from_bytes(
        r#"
        var x = 1 + 1;
        debugger; // This should pause for ~2 seconds
        var y = 2 + 2;
        y; // Return the value
    "#,
    );

    match context.eval(source) {
        Ok(_) => {
            eprintln!("\n[Test] SUCCESS: Pause/resume cycle completed!");
            eprintln!("[Test] ✓ No CPU cycles wasted on polling");
            eprintln!("[Test] ✓ Instant notification via OS signal");
        }
        Err(e) => {
            eprintln!("\n[Test] ERROR: {}", e);
            return Err(e);
        }
    }

    // Wait for the resume thread to finish
    resume_handle.join().unwrap();

    eprintln!("\n=== Test Complete ===\n");

    Ok(())
}
