//! `forward_store!`: implement an async store port for `&T` (several
//! services over one store) and/or `Arc<T>` (the API's writer and readers
//! share one store) by forwarding every listed method to the inner store.
//! Each method is written as `fn name(&self, args..) -> Output;`, where
//! `Output` is the returned future's output.

macro_rules! forward_store {
    ($trait:ident for [$($wrapper:ident),+] $methods:tt) => {
        $(forward_store!(@impl $wrapper $trait $methods);)+
    };
    (@impl ref $trait:ident $methods:tt) => {
        impl<T: $trait + Sync> $trait for &T {
            forward_store!(@methods $methods);
        }
    };
    (@impl arc $trait:ident $methods:tt) => {
        impl<T: $trait + Send + Sync> $trait for ::std::sync::Arc<T> {
            forward_store!(@methods $methods);
        }
    };
    (@methods { $(fn $method:ident(&self $(, $arg:ident: $ty:ty)* $(,)?) -> $out:ty;)+ }) => {
        $(
            fn $method(
                &self,
                $($arg: $ty),*
            ) -> impl Future<Output = $out> + Send {
                (**self).$method($($arg),*)
            }
        )+
    };
}

/// `forward_boxed!`: trait methods whose boxed future (`Pin<Box<dyn
/// Future>>` alias) only boxes another method's future, written as
/// `fn name(&self, args..) -> Fut<'_, T> = Self::inner;` (or
/// `= Trait::inner;` where the names clash); the arguments pass through.
macro_rules! forward_boxed {
    ($(fn $method:ident(&self $(, $arg:ident: $ty:ty)* $(,)?) -> $ret:ty = $target:path;)+) => {
        $(
            fn $method(&self $(, $arg: $ty)*) -> $ret {
                Box::pin($target(self $(, $arg)*))
            }
        )+
    };
}

pub(crate) use forward_boxed;
