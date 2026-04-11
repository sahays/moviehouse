use super::value::BValue;

/// Encode a `BValue` to bencode bytes.
pub fn encode(value: &BValue) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_to(value, &mut buf);
    buf
}

/// Encode a `BValue` directly into a buffer.
pub fn encode_to(value: &BValue, buf: &mut Vec<u8>) {
    match value {
        BValue::Int(n) => {
            buf.push(b'i');
            buf.extend_from_slice(n.to_string().as_bytes());
            buf.push(b'e');
        }
        BValue::Bytes(b) => {
            buf.extend_from_slice(b.len().to_string().as_bytes());
            buf.push(b':');
            buf.extend_from_slice(b);
        }
        BValue::List(items) => {
            buf.push(b'l');
            for item in items {
                encode_to(item, buf);
            }
            buf.push(b'e');
        }
        BValue::Dict(map) => {
            buf.push(b'd');
            // BTreeMap iterates in sorted order — required by bencode spec.
            for (key, val) in map {
                buf.extend_from_slice(key.len().to_string().as_bytes());
                buf.push(b':');
                buf.extend_from_slice(key);
                encode_to(val, buf);
            }
            buf.push(b'e');
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::bencode::decode;

    #[test]
    fn test_encode_int() {
        assert_eq!(encode(&BValue::Int(42)), b"i42e");
        assert_eq!(encode(&BValue::Int(0)), b"i0e");
        assert_eq!(encode(&BValue::Int(-1)), b"i-1e");
    }

    #[test]
    fn test_encode_bytes() {
        assert_eq!(encode(&BValue::Bytes(b"spam".to_vec())), b"4:spam");
        assert_eq!(encode(&BValue::Bytes(vec![])), b"0:");
    }

    #[test]
    fn test_encode_list() {
        let val = BValue::List(vec![BValue::Bytes(b"spam".to_vec()), BValue::Int(42)]);
        assert_eq!(encode(&val), b"l4:spami42ee");
    }

    #[test]
    fn test_roundtrip() {
        let inputs: &[&[u8]] = &[
            b"i42e",
            b"4:spam",
            b"l4:spami42ee",
            b"d3:cow3:moo4:spam4:eggse",
            b"d4:listli1ei2ei3ee4:name4:teste",
        ];
        for input in inputs {
            let decoded = decode::decode(input).unwrap();
            let reencoded = encode(&decoded);
            assert_eq!(reencoded, *input, "roundtrip failed for {input:?}");
        }
    }
}
