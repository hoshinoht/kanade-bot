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
