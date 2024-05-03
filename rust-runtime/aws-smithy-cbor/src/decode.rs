use std::borrow::Cow;

use aws_smithy_types::{Blob, DateTime};
use minicbor::decode::Error;

use crate::data::Type;

/// Provides functions for decoding a CBOR object with a known schema.
///
/// Although CBOR is a self-describing format, this decoder is tailored for cases where the schema
/// is known in advance. Therefore, the caller can determine which object key exists at the current
/// position by calling `str` method, and call the relevant function based on the predetermined schema
/// for that key. If an unexpected key is encountered, the caller can use the `skip` method to skip
/// over the element.
#[derive(Debug, Clone)]
pub struct Decoder<'b> {
    decoder: minicbor::Decoder<'b>,
}

/// When any of the decode methods are called they look for that particular data type at the current
/// position. If the CBOR data tag does not match the type, a `DeserializeError` is returned.
#[derive(Debug)]
pub struct DeserializeError {
    #[allow(dead_code)]
    _inner: Error,
}

impl std::fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self._inner.fmt(f)
    }
}

impl std::error::Error for DeserializeError {}

impl DeserializeError {
    pub(crate) fn new(inner: Error) -> Self {
        Self { _inner: inner }
    }

    /// More than one union variant was detected: `unexpected_type` was unexpected.
    pub fn unexpected_union_variant(unexpected_type: Type, at: usize) -> Self {
        Self {
            _inner: Error::type_mismatch(unexpected_type.into_minicbor_type())
                .with_message("encountered unexpected union variant; expected end of union")
                .at(at),
        }
    }

    /// More than one union variant was detected, but we never even got to parse the first one.
    pub fn mixed_union_variants(at: usize) -> Self {
        Self {
            _inner: Error::message("encountered mixed variants in union; expected end of union")
                .at(at),
        }
    }

    /// An unexpected type was encountered.
    // We handle this one when decoding sparse collections: we have to expect either a `null` or an
    // item, so we try decoding both.
    pub fn is_type_mismatch(&self) -> bool {
        self._inner.is_type_mismatch()
    }
}


/// Macro for delegating method calls to the decoder.
///
/// This macro generates wrapper methods for calling specific encoder methods on the decoder
/// and returning the result with error handling.
///
/// # Example
///
/// ```
/// delegate_method! {
///     /// Wrapper method for encoding method `encode_str` on the decoder.
///     encode_str_wrapper => encode_str(String);
///     /// Wrapper method for encoding method `encode_int` on the decoder.
///     encode_int_wrapper => encode_int(i32);
/// }
/// ```
macro_rules! delegate_method {
    ($($(#[$meta:meta])* $wrapper_name:ident => $encoder_name:ident($result_type:ty);)+) => {
        $(
            pub fn $wrapper_name(&mut self) -> Result<$result_type, DeserializeError> {
                self.decoder.$encoder_name().map_err(DeserializeError::new)
            }
        )+
    };
}

impl<'b> Decoder<'b> {
    pub fn new(bytes: &'b [u8]) -> Self {
        Self {
            decoder: minicbor::Decoder::new(bytes),
        }
    }

    pub fn datatype(&self) -> Result<Type, DeserializeError> {
        self.decoder
            .datatype()
            .map(Type::new)
            .map_err(DeserializeError::new)
    }

    delegate_method! {
        /// Skips the current CBOR element.
        skip => skip(());
        /// Reads a boolean at the current position.
        boolean => bool(bool);
        /// Reads a byte at the current position.
        byte => i8(i8);
        /// Reads a short at the current position.
        short => i16(i16);
        /// Reads a integer at the current position.
        integer => i32(i32);
        /// Reads a long at the current position.
        long => i64(i64);
        /// Reads a float at the current position.
        float => f32(f32);
        /// Reads a double at the current position.
        double => f64(f64);
        /// Reads a null CBOR element at the current position.
        null => null(());
        /// Returns the number of elements in a definite list. For indefinite lists it returns a `None`.
        list => array(Option<u64>);
        /// Returns the number of elements in a definite map. For indefinite map it returns a `None`.
        map => map(Option<u64>);
    }

    /// Returns the current position of the buffer, which will be decoded when any of the methods is called.
    pub fn position(&self) -> usize {
        self.decoder.position()
    }

    /// Returns a `cow::Borrowed(&str)` if the element at the current position in the buffer is a definite
    /// length string. Otherwise, it returns a `cow::Owned(String)` if the element at the current position is an
    /// indefinite-length string. An error is returned if the element is neither a definite length nor an
    /// indefinite-length string.
    pub fn str(&mut self) -> Result<Cow<'b, str>, DeserializeError> {
        let bookmark = self.decoder.position();
        match self.decoder.str() {
            Ok(str_value) => Ok(Cow::Borrowed(str_value)),
            Err(e) if e.is_type_mismatch() => {
                // Move the position back to the start of the CBOR element and then try
                // decoding it as a indefinite length string.
                self.decoder.set_position(bookmark);
                Ok(Cow::Owned(self.string()?))
            }
            Err(e) => Err(DeserializeError::new(e)),
        }
    }

    /// Allocates and returns a `String` if the element at the current position in the buffer is either a
    /// definite-length or an indefinite-length string. Otherwise, an error is returned if the element is not a string type.
    pub fn string(&mut self) -> Result<String, DeserializeError> {
        let mut iter = self.decoder.str_iter().map_err(DeserializeError::new)?;
        let head = iter.next();

        let decoded_string = match head {
            None => String::new(),
            Some(head) => {
                let mut combined_chunks = String::from(head.map_err(DeserializeError::new)?);
                for chunk in iter {
                    combined_chunks.push_str(chunk.map_err(DeserializeError::new)?);
                }
                combined_chunks
            }
        };

        Ok(decoded_string)
    }

    /// Returns a `blob` if the element at the current position in the buffer is a byte string. Otherwise,
    /// a `DeserializeError` error is returned.
    pub fn blob(&mut self) -> Result<Blob, DeserializeError> {
        let iter = self.decoder.bytes_iter().map_err(DeserializeError::new)?;
        let parts: Vec<&[u8]> = iter
            .collect::<Result<_, _>>()
            .map_err(DeserializeError::new)?;

        Ok(if parts.len() == 1 {
            Blob::new(parts[0]) // Directly convert &[u8] to Blob if there's only one part.
        } else {
            Blob::new(parts.concat()) // Concatenate all parts into a single Blob.
        })
    }

    /// Returns a `DateTime` if the element at the current position in the buffer is a `timestamp`. Otherwise,
    /// a `DeserializeError` error is returned.
    pub fn timestamp(&mut self) -> Result<DateTime, DeserializeError> {
        let tag = self.decoder.tag().map_err(DeserializeError::new)?;

        if !matches!(tag, minicbor::data::Tag::Timestamp) {
            Err(DeserializeError::new(Error::message(
                "expected timestamp tag",
            )))
        } else {
            let epoch_seconds = self.decoder.f64().map_err(DeserializeError::new)?;
            Ok(DateTime::from_secs_f64(epoch_seconds))
        }
    }
}

#[derive(Debug)]
pub struct ArrayIter<'a, 'b, T> {
    inner: minicbor::decode::ArrayIter<'a, 'b, T>,
}

impl<'a, 'b, T: minicbor::Decode<'b, ()>> Iterator for ArrayIter<'a, 'b, T> {
    type Item = Result<T, DeserializeError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .next()
            .map(|opt| opt.map_err(DeserializeError::new))
    }
}

#[derive(Debug)]
pub struct MapIter<'a, 'b, K, V> {
    inner: minicbor::decode::MapIter<'a, 'b, K, V>,
}

impl<'a, 'b, K, V> Iterator for MapIter<'a, 'b, K, V>
where
    K: minicbor::Decode<'b, ()>,
    V: minicbor::Decode<'b, ()>,
{
    type Item = Result<(K, V), DeserializeError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .next()
            .map(|opt| opt.map_err(DeserializeError::new))
    }
}

pub fn set_optional<B, F>(builder: B, decoder: &mut Decoder, f: F) -> Result<B, DeserializeError>
where
    F: Fn(B, &mut Decoder) -> Result<B, DeserializeError>,
{
    match decoder.datatype()? {
        crate::data::Type::Null => {
            decoder.null()?;
            Ok(builder)
        }
        _ => f(builder, decoder),
    }
}

#[cfg(test)]
mod tests {
    use crate::Decoder;

    #[test]
    fn test_definite_str_is_cow_borrowed() {
        // Definite length key `thisIsAKey`.
        let definite_bytes = [
            0x6a, 0x74, 0x68, 0x69, 0x73, 0x49, 0x73, 0x41, 0x4b, 0x65, 0x79,
        ];
        let mut decoder = Decoder::new(&definite_bytes);
        let member = decoder.str().expect("could not decode str");
        assert_eq!(member, "thisIsAKey");
        assert!(matches!(member, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn test_indefinite_str_is_cow_owned() {
        // Indefinite length key `this`, `Is`, `A` and `Key`.
        let indefinite_bytes = [
            0x7f, 0x64, 0x74, 0x68, 0x69, 0x73, 0x62, 0x49, 0x73, 0x61, 0x41, 0x63, 0x4b, 0x65,
            0x79, 0xff,
        ];
        let mut decoder = Decoder::new(&indefinite_bytes);
        let member = decoder.str().expect("could not decode str");
        assert_eq!(member, "thisIsAKey");
        assert!(matches!(member, std::borrow::Cow::Owned(_)));
    }

    #[test]
    fn test_empty_str_works() {
        let bytes = [0x60];
        let mut decoder = Decoder::new(&bytes);
        let member = decoder.str().expect("could not decode empty str");
        assert_eq!(member, "");
    }

    #[test]
    fn test_empty_blob_works() {
        let bytes = [0x40];
        let mut decoder = Decoder::new(&bytes);
        let member = decoder.blob().expect("could not decode an empty blob");
        assert_eq!(member, aws_smithy_types::Blob::new(&[]));
    }

    #[test]
    fn test_indefinite_length_blob() {
        // Indefinite length blob containing bytes corresponding to `indefinite-byte, chunked, on each comma`.
        // https://cbor.nemo157.com/#type=hex&value=bf69626c6f6256616c75655f50696e646566696e6974652d627974652c49206368756e6b65642c4e206f6e206561636820636f6d6d61ffff
        let indefinite_bytes = [
            0x5f, 0x50, 0x69, 0x6e, 0x64, 0x65, 0x66, 0x69, 0x6e, 0x69, 0x74, 0x65, 0x2d, 0x62,
            0x79, 0x74, 0x65, 0x2c, 0x49, 0x20, 0x63, 0x68, 0x75, 0x6e, 0x6b, 0x65, 0x64, 0x2c,
            0x4e, 0x20, 0x6f, 0x6e, 0x20, 0x65, 0x61, 0x63, 0x68, 0x20, 0x63, 0x6f, 0x6d, 0x6d,
            0x61, 0xff,
        ];
        let mut decoder = Decoder::new(&indefinite_bytes);
        let member = decoder.blob().expect("could not decode blob");
        assert_eq!(
            member,
            aws_smithy_types::Blob::new("indefinite-byte, chunked, on each comma".as_bytes())
        );
    }
}
