//! Bounded JSON decoding that rejects duplicate keys before constructing a map.

use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, value::RawValue};
use std::fmt;

#[derive(Default)]
struct Budget {
    values: usize,
    containers: usize,
}

struct Checked<'a> {
    budget: &'a mut Budget,
    depth: usize,
}

pub(crate) fn decode(text: &str) -> Result<Value, serde_json::Error> {
    let mut budget = Budget::default();
    let mut decoder = serde_json::Deserializer::from_str(text);
    let value = Checked {
        budget: &mut budget,
        depth: 0,
    }
    .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

impl<'de> DeserializeSeed<'de> for Checked<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        self.budget.values += 1;
        if self.budget.values > 100_000 {
            return Err(de::Error::custom("too many data values (limit 100000)"));
        }
        // RawValue's scanner uses an explicit stack. We enforce our own container
        // depth before recursively decoding, including unknown extension fields.
        let raw = <&RawValue>::deserialize(decoder)?;
        let text = raw.get();
        if matches!(text.as_bytes().first(), Some(b'{' | b'[')) {
            self.budget.containers += 1;
            if self.depth > 128 || self.budget.containers > 10_000 {
                return Err(de::Error::custom(
                    "data nesting exceeds 128 levels or 10000 containers",
                ));
            }
            let mut nested = serde_json::Deserializer::from_str(text);
            nested.deserialize_any(self).map_err(de::Error::custom)
        } else {
            let value: Value = serde_json::from_str(text).map_err(de::Error::custom)?;
            // Python permits arbitrary integers, but not non-finite floats.
            if let Value::Number(n) = &value
                && text.contains(['.', 'e', 'E'])
                && !n.as_f64().is_some_and(f64::is_finite)
            {
                return Err(de::Error::custom("non-finite JSON number"));
            }
            Ok(value)
        }
    }
}

impl<'de> Visitor<'de> for Checked<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON container")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut fields = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            if fields.contains_key(&key) {
                return Err(de::Error::custom("JSON object contains a duplicate field"));
            }
            let value = access.next_value_seed(Checked {
                budget: self.budget,
                depth: self.depth + 1,
            })?;
            fields.insert(key, value);
        }
        Ok(Value::Object(fields))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = access.next_element_seed(Checked {
            budget: self.budget,
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
}
