mod sealed {
    pub trait Sealed {}
}

/// Argument tuples accepted by [`blocking_call`](crate::blocking_call),
/// implemented for tuples of arity 0–4 over the wasm ABI scalars (`u32`,
/// `i32`, `usize`, `f64`), each mapping to its unit-returning
/// `extern "C-unwind" fn(...)` pointer type (`(f64,)` →
/// `extern "C-unwind" fn(f64)`). `C-unwind` so that panics raised before
/// the suspension point (denial panics, shim validation) unwind with
/// destructors through the bracket instead of aborting at the boundary;
/// panics after the foreign import returns remain a contract violation.
///
/// Scalars only, deliberately: no references (nothing borrowed can be
/// smuggled into a frame the callee's JS might inspect after suspension —
/// passing a pointer is an explicit `as usize` act in consumer glue),
/// everything `Copy` and consumed before the suspension, and the unit
/// return is enforced by the `Fn` type.
pub trait BlockingArgs: sealed::Sealed + Copy {
    /// The unit-returning `extern "C"` fn-pointer type for this arity.
    type Fn: Copy;

    #[doc(hidden)]
    fn call(f: Self::Fn, args: Self);
}

macro_rules! impl_args {
    (@ [$($n:ident : $t:ty,)*]) => {
        impl sealed::Sealed for ($($t,)*) {}
        impl BlockingArgs for ($($t,)*) {
            type Fn = extern "C-unwind" fn($($t),*);
            #[inline(always)]
            fn call(f: Self::Fn, args: Self) {
                #[allow(non_snake_case, unused_variables)]
                let ($($n,)*) = args;
                f($($n),*)
            }
        }
    };
    (@ [$($acc:ident : $t:ty,)*] $n:ident $($rest:ident)*) => {
        impl_args!(@ [$($acc : $t,)* $n: u32,] $($rest)*);
        impl_args!(@ [$($acc : $t,)* $n: i32,] $($rest)*);
        impl_args!(@ [$($acc : $t,)* $n: usize,] $($rest)*);
        impl_args!(@ [$($acc : $t,)* $n: f64,] $($rest)*);
    };
    ($($n:ident)*) => {
        impl_args!(@ [] $($n)*);
    };
}

impl_args!();
impl_args!(a);
impl_args!(a b);
impl_args!(a b c);
impl_args!(a b c d);
