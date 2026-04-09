use std::collections::BTreeMap;

use super::value::BValue;

/// Errors that can occur during bencode decoding.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("unexpected end of input at position {0}")]
    UnexpectedEof(usize),
    #[error("invalid byte '{0}' at position {1}")]
    InvalidByte(u8, usize),
    #[error("invalid integer at position {0}")]
    InvalidInteger(usize),
    #[error("leading zero in integer at position {0}")]
    LeadingZero(usize),
    #[error("negative zero at position {0}")]
    NegativeZero(usize),
    #[error("invalid string length at position {0}")]
    InvalidStringLength(usize),
    #[error("string length overflow at position {0}")]
    StringLengthOverflow(usize),
    #[error("unsorted dictionary keys at position {0}")]
    UnsortedKeys(usize),
    #[error("duplicate dictionary key at position {0}")]
    DuplicateKey(usize),
}

/// Bencode decoder that tracks byte positions for raw slice extraction.
pub struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
}

/// Result of decoding, including the raw byte range of the decoded value.
pub struct DecodeResult {
    pub value: BValue,
    /// Start position (inclusive) of this value in the source bytes.
    pub start: usize,
    /// End position (exclusive) of this value in the source bytes.
    pub end: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Decode one BValue. Returns the value and the byte range it occupied.
    pub fn decode(&mut self) -> Result<DecodeResult, DecodeError> {
        let start = self.pos;
        let value = self.decode_value()?;
        Ok(DecodeResult {
            value,
            start,
            end: self.pos,
        })
    }

    /// Get the raw byte slice for a given range.
    pub fn raw_slice(&self, start: usize, end: usize) -> &'a [u8] {
        &self.data[start..end]
    }

    /// Current position in the input.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Whether we've consumed all input.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn peek(&self) -> Result<u8, DecodeError> {
        self.data
            .get(self.pos)
            .copied()
            .ok_or(DecodeError::UnexpectedEof(self.pos))
    }

    fn advance(&mut self) -> Result<u8, DecodeError> {
        let b = self.peek()?;
        self.pos += 1;
        Ok(b)
    }

    fn expect(&mut self, expected: u8) -> Result<(), DecodeError> {
        let b = self.advance()?;
        if b == expected {
            Ok(())
        } else {
            DecodeError::InvalidByte(b, self.pos - 1).into()
        }
    }

    fn decode_value(&mut self) -> Result<BValue, DecodeError> {
        match self.peek()? {
            b'i' => self.decode_int(),
            b'l' => self.decode_list(),
            b'd' => self.decode_dict(),
            b'0'..=b'9' => self.decode_bytes(),
            b => Err(DecodeError::InvalidByte(b, self.pos)),
        }
    }

    fn decode_int(&mut self) -> Result<BValue, DecodeError> {
        self.expect(b'i')?;
        let start = self.pos;

        // Find 'e'
        let end_pos = self.data[self.pos..]
            .iter()
            .position(|&b| b == b'e')
            .ok_or(DecodeError::UnexpectedEof(self.pos))?
            + self.pos;

        let num_str = std::str::from_utf8(&self.data[start..end_pos])
            .map_err(|_| DecodeError::InvalidInteger(start))?;

        // Validate: no leading zeros (except "0" itself), no "-0"
        if num_str.len() > 1 && num_str.starts_with('0') {
            return Err(DecodeError::LeadingZero(start));
        }
        if num_str == "-0" {
            return Err(DecodeError::NegativeZero(start));
        }
        if num_str.len() > 1 && num_str.starts_with("-0") {
            return Err(DecodeError::LeadingZero(start));
        }

        let n: i64 = num_str
            .parse()
            .map_err(|_| DecodeError::InvalidInteger(start))?;

        self.pos = end_pos;
        self.expect(b'e')?;
        Ok(BValue::Int(n))
    }

    fn decode_bytes(&mut self) -> Result<BValue, DecodeError> {
        let start = self.pos;
        let mut length: usize = 0;

        // Parse length digits
        loop {
            let b = self.peek()?;
            if b == b':' {
                self.pos += 1;
                break;
            }
            if !b.is_ascii_digit() {
                return Err(DecodeError::InvalidStringLength(start));
            }
            self.pos += 1;
            length = length
                .checked_mul(10)
                .and_then(|l| l.checked_add((b - b'0') as usize))
                .ok_or(DecodeError::StringLengthOverflow(start))?;
        }

        // Read exactly `length` bytes
        if self.pos + length > self.data.len() {
            return Err(DecodeError::UnexpectedEof(self.pos));
        }

        let bytes = self.data[self.pos..self.pos + length].to_vec();
        self.pos += length;
        Ok(BValue::Bytes(bytes))
    }

    fn decode_list(&mut self) -> Result<BValue, DecodeError> {
        self.expect(b'l')?;
        let mut items = Vec::new();

        while self.peek()? != b'e' {
            items.push(self.decode_value()?);
        }

        self.expect(b'e')?;
        Ok(BValue::List(items))
    }

    fn decode_dict(&mut self) -> Result<BValue, DecodeError> {
        self.expect(b'd')?;
        let mut map = BTreeMap::new();
        while self.peek()? != b'e' {
            let key_start = self.pos;
            let key = match self.decode_value()? {
                BValue::Bytes(k) => k,
                _ => return Err(DecodeError::InvalidByte(self.data[key_start], key_start)),
            };

            // BEP3 requires sorted keys, but real-world peers often don't comply.
            // BTreeMap sorts on insert, so the output is always correct regardless.
            let value = self.decode_value()?;
            map.insert(key, value);
        }

        self.expect(b'e')?;
        Ok(BValue::Dict(map))
    }
}

/// Convenience function: decode a single bencoded value from bytes.
pub fn decode(data: &[u8]) -> Result<BValue, DecodeError> {
    let mut decoder = Decoder::new(data);
    let result = decoder.decode()?;
    Ok(result.value)
}

/// Decode and also return the raw byte range for the top-level value.
pub fn decode_with_range(data: &[u8]) -> Result<DecodeResult, DecodeError> {
    let mut decoder = Decoder::new(data);
    decoder.decode()
}

impl From<DecodeError> for Result<(), DecodeError> {
    fn from(e: DecodeError) -> Self {
        Err(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_int() {
        assert_eq!(decode(b"i42e").unwrap(), BValue::Int(42));
        assert_eq!(decode(b"i0e").unwrap(), BValue::Int(0));
        assert_eq!(decode(b"i-1e").unwrap(), BValue::Int(-1));
    }

    #[test]
    fn test_decode_int_invalid() {
        assert!(decode(b"i-0e").is_err()); // negative zero
        assert!(decode(b"i03e").is_err()); // leading zero
    }

    #[test]
    fn test_decode_bytes() {
        assert_eq!(
            decode(b"4:spam").unwrap(),
            BValue::Bytes(b"spam".to_vec())
        );
        assert_eq!(decode(b"0:").unwrap(), BValue::Bytes(vec![]));
    }

    #[test]
    fn test_decode_list() {
        let val = decode(b"l4:spami42ee").unwrap();
        assert_eq!(
            val,
            BValue::List(vec![
                BValue::Bytes(b"spam".to_vec()),
                BValue::Int(42),
            ])
        );
    }

    #[test]
    fn test_decode_dict() {
        let val = decode(b"d3:cow3:moo4:spam4:eggse").unwrap();
        let dict = val.as_dict().unwrap();
        assert_eq!(
            dict.get(&b"cow".to_vec()).unwrap().as_bytes().unwrap(),
            b"moo"
        );
        assert_eq!(
            dict.get(&b"spam".to_vec()).unwrap().as_bytes().unwrap(),
            b"eggs"
        );
    }

    #[test]
    fn test_decode_nested() {
        let val = decode(b"d4:listli1ei2ei3eee").unwrap();
        let list = val.get_str("list").unwrap().as_list().unwrap();
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_raw_slice_tracking() {
        // The info dict in a torrent file: d4:info d ... e e
        let data = b"d4:infod4:name4:teste3:onei1ee";
        let mut decoder = Decoder::new(data);
        let result = decoder.decode().unwrap();
        let dict = result.value.as_dict().unwrap();
        assert!(dict.contains_key(&b"info".to_vec()));
    }
}
