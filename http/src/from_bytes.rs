use std::{fmt, str};

/// Analogous trait to [`FromStr`].
///
/// The main use case for this trait in [`Header::parse`]. Because of this the
/// implementations should expect the `value`s passed to be ASCII/UTF-8, but
/// this not true in all cases.
///
/// [`FromStr`]: std::str::FromStr
/// [`Header::parse`]: crate::Header::parse
pub trait FromBytes<'a>: Sized {
    /// Error returned by parsing the bytes.
    type Err;

    /// Parse the `value`.
    fn from_bytes(value: &'a [u8]) -> Result<Self, Self::Err>;
}

#[derive(Debug)]
pub struct ParseIntError;

impl fmt::Display for ParseIntError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid integer")
    }
}

macro_rules! int_impl {
    ($( $ty: ty ),+) => {
        $(
        impl FromBytes<'_> for $ty {
            type Err = ParseIntError;

            fn from_bytes(src: &[u8]) -> Result<Self, Self::Err> {
                if src.is_empty() {
                    return Err(ParseIntError);
                }

                let mut value: $ty = 0;
                for b in src.iter().copied() {
                    if b >= b'0' && b <= b'9' {
                        if value >= (<$ty>::MAX / 10) {
                            // Overflow.
                            return Err(ParseIntError);
                        }
                        value = (value * 10) + (b - b'0') as $ty;
                    } else {
                        return Err(ParseIntError);
                    }
                }
                Ok(value)
            }
        }
        )+
    };
}

int_impl!(u8, u16, u32, u64, usize);

impl<'a> FromBytes<'a> for &'a str {
    type Err = str::Utf8Error;

    fn from_bytes(src: &'a [u8]) -> Result<Self, Self::Err> {
        str::from_utf8(src)
    }
}