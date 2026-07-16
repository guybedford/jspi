use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::mem::MaybeUninit;
use std::ptr;

use crate::BlockingArgs;

unsafe extern "C" {
    fn emscripten_stack_get_base() -> usize;
    fn emscripten_stack_get_current() -> usize;
    fn _emscripten_stack_restore(sp: usize);
}

// WebAssembly stack shifts downwards towards 0
// 
// Stack ranges are thus of the form [TOP..BASE]
// 
// When suspension happens, we store the stack range from [TOP..JSPI_SUSPEND_TOP],
// where JSPI_SUSPEND_TOP is the stack of stack tops populated by nested enter_promising
// calls.
// 
// The stored stack is given a unique STACK_ID, and saved into the STACKS map for that ID.
// 
// Stacks may be resumed in arbitrary order. When resuming the saved stack is written
// back into the stack, we do not implements Chatham's optimization for avoiding the copy.
// 
// See https://blog.pyodide.org/posts/jspi-with-c-runtime/ for more background.
//

thread_local! {
    // Tracks the JSPI stack pointer of nested enter_promising
    static JSPI_SUSPEND_TOP: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    // ID counter for stored stack
    static STACK_ID: Cell<usize> = const { Cell::new(0) };
    // Stores of the suspended JSPI stacks currently suspended, keyed by STACK_ID to (STACK_TOP, DATA)
    static STACKS: RefCell<BTreeMap<usize, (usize, Box<[MaybeUninit<u8>]>)>> = const { RefCell::new(BTreeMap::new()) };
}

/// Creates a new JSPI promising scope.
///
/// `Send + 'static` ensures no borrow from outside the scope can be held
/// across the blocking suspension points within it.
///
/// # Safety
///
/// The caller asserts that this is the first statement within a direct
/// `WebAssembly.promising` function.
///
pub unsafe fn enter_promising<R>(f: impl FnOnce() + Send + 'static) {
    let _stack = PromisingGuard::enter();
    f();
}

/// Perform a blocking JSPI suspending call
///
/// Supports arbitrary arity through the `BlockingArgs` generic on
/// arbitrary tuple arguments.
///
/// # Safety
///
/// Same as any foreign function in Rust.
///
pub unsafe fn blocking_call<A: BlockingArgs>(f: A::Fn, args: A) {
    let _suspended_stack = SavedStack::suspend();
    A::call(f, args);
}


/// PromisingGuard is used to capture the entry and exit of a WebAssembly.promising
/// enter_promising call, to track the JSPI_SUSPEND_TOP stack created.
/// The pushed value will be mutated by subsequent suspensions.
struct PromisingGuard;

impl PromisingGuard {
    fn enter() -> PromisingGuard {
        JSPI_SUSPEND_TOP.with(|ptr| {
            ptr.borrow_mut()
                .push(unsafe { emscripten_stack_get_current() })
        });
        PromisingGuard
    }
}

impl Drop for PromisingGuard {
    fn drop(&mut self) {
        JSPI_SUSPEND_TOP.with(|ptr| ptr.borrow_mut().pop());
    }
}

/// SavedStack is used to create a stack suspension for WebAssembly.Suspending
/// function calls.
/// 
/// On resume, the JSPI_SUSPEND_TOP will be updated to the new stack top, before
/// restoring the stack.
struct SavedStack(usize);

impl SavedStack {
    fn suspend() -> SavedStack {
        // Range is [TOP..BASE]
        let suspend_range = unsafe {
            emscripten_stack_get_current()
                ..STACK_BASE
                    .get()
                    .expect("Expected to be in an enter_promising context")
        };
        let mut buf = Box::new_uninit_slice(suspend_range.len());
        unsafe {
            ptr::copy_nonoverlapping(
                suspend_range.end as *const MaybeUninit<u8>,
                buf.as_mut_ptr(),
                suspend_range.len(),
            );
        }
        let stack_id = STACK_ID.with(|id| {
            let stack_id = id.get() + 1;
            id.set(stack_id);
            stack_id
        });
        // Restore the stack to the *base*
        unsafe { _emscripten_stack_restore(suspend_range.end) };
        STACKS.with(|stacks| stacks.borrow_mut().insert(stack_id, buf));
        SavedStack(stack_id)
    }
}

impl Drop for SavedStack {
    fn drop(&mut self) {
        let new_stack_top = unsafe { emscripten_stack_get_current() };
        let buf = STACKS.with(|stacks| {
            stacks
                .borrow_mut()
                .remove(&self.0)
                .expect("Unable to find suspended stack on resume")
        });
        let low = new_stack_top - buf.len();
        unsafe {
            ptr::copy_nonoverlapping(buf.as_ptr(), low as *mut MaybeUninit<u8>, buf.len());
            _emscripten_stack_restore(low);
        }
    }
}
