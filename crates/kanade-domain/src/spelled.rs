//! `spelled!`: a fieldless enum with one stored (and API) spelling per
//! variant, `ALL` in declaration order, `as_str`, `parse` and `Display`.
//! `parse` returns `Option`, or with `Name else Error::Variant { .. }`
//! `Err(Error::Variant(text))` for an unknown spelling.

#[macro_export]
macro_rules! spelled {
    (
        $(#[$meta:meta])*
        $name:ident $(else $error:ident::$unknown:ident)?
        { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            $crate::spelled!(@parse $($error $unknown)?);
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
    (@parse) => {
        pub fn parse(value: &str) -> Option<Self> {
            Self::ALL.iter().copied().find(|item| item.as_str() == value)
        }
    };
    (@parse $error:ident $unknown:ident) => {
        /// # Errors
        #[doc = concat!(
            "[`", stringify!($error), "::", stringify!($unknown), "`] for any other text."
        )]
        pub fn parse(value: &str) -> Result<Self, $error> {
            Self::ALL
                .iter()
                .copied()
                .find(|item| item.as_str() == value)
                .ok_or_else(|| $error::$unknown(value.to_owned()))
        }
    };
}
