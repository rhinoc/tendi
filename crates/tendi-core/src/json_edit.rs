use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug)]
struct JsonNode {
    start: usize,
    end: usize,
    kind: JsonNodeKind,
}

#[derive(Debug)]
enum JsonNodeKind {
    Object(Vec<JsonMember>),
    Array(Vec<JsonNode>),
    Scalar,
}

#[derive(Debug)]
struct JsonMember {
    key: String,
    key_start: usize,
    value: JsonNode,
}

#[derive(Debug)]
struct JsonEdit {
    start: usize,
    end: usize,
    replacement: String,
}

pub(crate) fn patch_json_text(source: &str, before: &Value, after: &Value) -> Result<String> {
    if before == after {
        return Ok(source.to_string());
    }

    let mut parser = JsonParser::new(source);
    let root = parser.parse_document()?;
    let mut edits = Vec::new();
    collect_edits(source, &root, before, after, &mut edits)?;
    if edits.is_empty() {
        bail!("JSON change could not be mapped to source text")
    }

    edits.sort_by(|left, right| right.start.cmp(&left.start));
    let mut result = source.to_string();
    let mut next_start = source.len();
    for edit in edits {
        if edit.end > next_start {
            bail!("JSON source edits overlap")
        }
        result.replace_range(edit.start..edit.end, &edit.replacement);
        next_start = edit.start;
    }
    Ok(result)
}

fn collect_edits(
    source: &str,
    node: &JsonNode,
    before: &Value,
    after: &Value,
    edits: &mut Vec<JsonEdit>,
) -> Result<()> {
    match (&node.kind, before, after) {
        (JsonNodeKind::Object(members), Value::Object(before), Value::Object(after)) => {
            for (index, member) in members.iter().enumerate() {
                let before_value = before
                    .get(&member.key)
                    .with_context(|| format!("JSON source key {} is missing", member.key))?;
                match after.get(&member.key) {
                    Some(after_value) => {
                        collect_edits(source, &member.value, before_value, after_value, edits)?;
                    }
                    None => edits.push(remove_object_member(source, members, index)),
                }
            }
            for (key, value) in after {
                if !before.contains_key(key) {
                    edits.push(insert_object_member(source, node, members, key, value)?);
                }
            }
            Ok(())
        }
        (JsonNodeKind::Array(items), Value::Array(before), Value::Array(after)) => {
            if before.len() == after.len() {
                for ((before_value, after_value), item) in before.iter().zip(after).zip(items) {
                    collect_edits(source, item, before_value, after_value, edits)?;
                }
                return Ok(());
            }

            if after.len() == before.len() + 1 {
                if let Some(index) = inserted_array_index(before, after) {
                    edits.push(insert_array_item(
                        source,
                        node,
                        items,
                        index,
                        &after[index],
                    )?);
                    return Ok(());
                }
            }
            if before.len() == after.len() + 1 {
                if let Some(index) = removed_array_index(before, after) {
                    edits.push(remove_array_item(items, index));
                    return Ok(());
                }
            }

            edits.push(JsonEdit {
                start: node.start,
                end: node.end,
                replacement: serde_json::to_string(after)?,
            });
            Ok(())
        }
        (JsonNodeKind::Scalar, _, _) => {
            edits.push(JsonEdit {
                start: node.start,
                end: node.end,
                replacement: serde_json::to_string(after)?,
            });
            Ok(())
        }
        _ => {
            edits.push(JsonEdit {
                start: node.start,
                end: node.end,
                replacement: serde_json::to_string(after)?,
            });
            Ok(())
        }
    }
}

fn inserted_array_index(before: &[Value], after: &[Value]) -> Option<usize> {
    (0..after.len())
        .find(|index| before[..*index] == after[..*index] && before[*index..] == after[index + 1..])
}

fn removed_array_index(before: &[Value], after: &[Value]) -> Option<usize> {
    (0..before.len())
        .find(|index| before[..*index] == after[..*index] && before[index + 1..] == after[*index..])
}

fn remove_object_member(source: &str, members: &[JsonMember], index: usize) -> JsonEdit {
    let member = &members[index];
    let (start, end) = if index + 1 < members.len() {
        (member.key_start, members[index + 1].key_start)
    } else if index > 0 {
        (members[index - 1].value.end, member.value.end)
    } else {
        (member.key_start, member.value.end)
    };
    JsonEdit {
        start,
        end,
        replacement: if index == 0 && members.len() == 1 {
            source[start..start].to_string()
        } else {
            String::new()
        },
    }
}

fn insert_object_member(
    source: &str,
    node: &JsonNode,
    members: &[JsonMember],
    key: &str,
    value: &Value,
) -> Result<JsonEdit> {
    let key = serde_json::to_string(key)?;
    let value = serde_json::to_string(value)?;
    if members.is_empty() {
        let inner = &source[node.start + 1..node.end - 1];
        if let Some(newline) = detect_newline(inner) {
            let closing_indent = indentation_before(source, node.end - 1);
            return Ok(JsonEdit {
                start: node.start + 1,
                end: node.start + 1,
                replacement: format!(
                    "{newline}{}{}: {value}{inner}",
                    format!("{closing_indent}  "),
                    key
                ),
            });
        }
        return Ok(JsonEdit {
            start: node.start + 1,
            end: node.start + 1,
            replacement: format!("{key}: {value}"),
        });
    }

    let last = members.last().expect("non-empty JSON object");
    let separator = source[last.value.end..node.end - 1].to_string();
    if let Some(newline) = detect_newline(&separator) {
        let indent = indentation_before(source, last.key_start);
        Ok(JsonEdit {
            start: last.value.end,
            end: last.value.end,
            replacement: format!(",{newline}{indent}{key}: {value}"),
        })
    } else {
        Ok(JsonEdit {
            start: last.value.end,
            end: last.value.end,
            replacement: format!(", {key}: {value}"),
        })
    }
}

fn insert_array_item(
    source: &str,
    node: &JsonNode,
    items: &[JsonNode],
    index: usize,
    value: &Value,
) -> Result<JsonEdit> {
    let value = serde_json::to_string(value)?;
    if items.is_empty() {
        let inner = &source[node.start + 1..node.end - 1];
        if let Some(newline) = detect_newline(inner) {
            let closing_indent = indentation_before(source, node.end - 1);
            return Ok(JsonEdit {
                start: node.start + 1,
                end: node.start + 1,
                replacement: format!(
                    "{newline}{}{}{}",
                    format!("{closing_indent}  "),
                    value,
                    inner
                ),
            });
        }
        return Ok(JsonEdit {
            start: node.start + 1,
            end: node.start + 1,
            replacement: value,
        });
    }

    if index == items.len() {
        let last = items.last().expect("non-empty JSON array");
        let suffix = &source[last.end..node.end - 1];
        if let Some(newline) = detect_newline(suffix) {
            let indent = indentation_before(source, last.start);
            return Ok(JsonEdit {
                start: last.end,
                end: last.end,
                replacement: format!(",{newline}{indent}{value}"),
            });
        }
        return Ok(JsonEdit {
            start: last.end,
            end: last.end,
            replacement: format!(", {value}"),
        });
    }

    let separator_start = if index == 0 {
        node.start + 1
    } else {
        items[index - 1].end
    };
    let separator = source[separator_start..items[index].start].to_string();
    Ok(JsonEdit {
        start: items[index].start,
        end: items[index].start,
        replacement: format!("{value},{separator}"),
    })
}

fn remove_array_item(items: &[JsonNode], index: usize) -> JsonEdit {
    let item = &items[index];
    let (start, end) = if index + 1 < items.len() {
        (item.start, items[index + 1].start)
    } else if index > 0 {
        (items[index - 1].end, item.end)
    } else {
        (item.start, item.end)
    };
    JsonEdit {
        start,
        end,
        replacement: String::new(),
    }
}

fn detect_newline(text: &str) -> Option<&'static str> {
    if text.contains("\r\n") {
        Some("\r\n")
    } else if text.contains('\n') {
        Some("\n")
    } else {
        None
    }
}

fn indentation_before(source: &str, position: usize) -> String {
    let line_start = source[..position].rfind('\n').map_or(0, |index| index + 1);
    source[line_start..position]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}

struct JsonParser<'a> {
    source: &'a str,
    bytes: &'a [u8],
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            position: 0,
        }
    }

    fn parse_document(&mut self) -> Result<JsonNode> {
        self.skip_whitespace();
        let root = self.parse_value()?;
        self.skip_whitespace();
        if self.position != self.bytes.len() {
            bail!("unexpected JSON text after root value")
        }
        Ok(root)
    }

    fn parse_value(&mut self) -> Result<JsonNode> {
        self.skip_whitespace();
        let start = self.position;
        let byte = *self
            .bytes
            .get(self.position)
            .context("JSON value is missing")?;
        let kind = match byte {
            b'{' => JsonNodeKind::Object(self.parse_object()?),
            b'[' => JsonNodeKind::Array(self.parse_array()?),
            b'"' => {
                self.parse_string()?;
                JsonNodeKind::Scalar
            }
            b't' => {
                self.parse_literal("true")?;
                JsonNodeKind::Scalar
            }
            b'f' => {
                self.parse_literal("false")?;
                JsonNodeKind::Scalar
            }
            b'n' => {
                self.parse_literal("null")?;
                JsonNodeKind::Scalar
            }
            b'-' | b'0'..=b'9' => {
                self.parse_number()?;
                JsonNodeKind::Scalar
            }
            _ => bail!("invalid JSON value at byte {start}"),
        };
        Ok(JsonNode {
            start,
            end: self.position,
            kind,
        })
    }

    fn parse_object(&mut self) -> Result<Vec<JsonMember>> {
        self.position += 1;
        self.skip_whitespace();
        let mut members = Vec::new();
        let mut keys = HashSet::new();
        if self.consume_if(b'}') {
            return Ok(members);
        }

        loop {
            self.skip_whitespace();
            let key_start = self.position;
            let key = self.parse_string()?;
            if !keys.insert(key.clone()) {
                bail!("duplicate JSON object key {key}")
            }
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.parse_value()?;
            members.push(JsonMember {
                key,
                key_start,
                value,
            });
            self.skip_whitespace();
            if self.consume_if(b'}') {
                return Ok(members);
            }
            self.expect(b',')?;
        }
    }

    fn parse_array(&mut self) -> Result<Vec<JsonNode>> {
        self.position += 1;
        self.skip_whitespace();
        let mut items = Vec::new();
        if self.consume_if(b']') {
            return Ok(items);
        }

        loop {
            items.push(self.parse_value()?);
            self.skip_whitespace();
            if self.consume_if(b']') {
                return Ok(items);
            }
            self.expect(b',')?;
        }
    }

    fn parse_string(&mut self) -> Result<String> {
        let start = self.position;
        self.expect(b'"')?;
        while let Some(byte) = self.bytes.get(self.position).copied() {
            match byte {
                b'"' => {
                    self.position += 1;
                    return serde_json::from_str(&self.source[start..self.position])
                        .context("invalid JSON string");
                }
                b'\\' => {
                    self.position += 1;
                    self.position += 1;
                }
                byte if byte < 0x20 => bail!("control character in JSON string"),
                _ => self.position += 1,
            }
        }
        bail!("unterminated JSON string")
    }

    fn parse_literal(&mut self, literal: &str) -> Result<()> {
        let end = self.position + literal.len();
        if self.source.get(self.position..end) != Some(literal) {
            bail!("invalid JSON literal")
        }
        self.position = end;
        Ok(())
    }

    fn parse_number(&mut self) -> Result<()> {
        let start = self.position;
        while let Some(byte) = self.bytes.get(self.position).copied() {
            if matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}') {
                break;
            }
            self.position += 1;
        }
        let number = &self.source[start..self.position];
        let value = serde_json::from_str::<Value>(number).context("invalid JSON number")?;
        if !value.is_number() {
            bail!("JSON token is not a number")
        }
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<()> {
        if self.consume_if(expected) {
            Ok(())
        } else {
            bail!(
                "expected JSON byte {:?} at {}",
                expected as char,
                self.position
            )
        }
    }

    fn consume_if(&mut self, expected: u8) -> bool {
        if self.bytes.get(self.position) == Some(&expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::patch_json_text;

    #[test]
    fn patches_nested_json_scalar_without_reformatting_siblings() {
        let source =
            "{\r\n  \"z\": 1,\r\n  \"nested\": { \"enabled\": false, \"name\": \"keep\" }\r\n}\r\n";
        let before = json!({"z": 1, "nested": {"enabled": false, "name": "keep"}});
        let after = json!({"z": 1, "nested": {"enabled": true, "name": "keep"}});

        assert_eq!(
            patch_json_text(source, &before, &after).unwrap(),
            "{\r\n  \"z\": 1,\r\n  \"nested\": { \"enabled\": true, \"name\": \"keep\" }\r\n}\r\n"
        );
    }

    #[test]
    fn patches_json_array_insertion_without_reformatting_siblings() {
        let source = "{\n  \"keep\": true,\n  \"items\": [\n    {\"name\": \"one\"}\n  ]\n}\n";
        let before = json!({"keep": true, "items": [{"name": "one"}]});
        let after = json!({
            "keep": true,
            "items": [{"name": "one"}, {"name": "two"}]
        });

        assert_eq!(
            patch_json_text(source, &before, &after).unwrap(),
            "{\n  \"keep\": true,\n  \"items\": [\n    {\"name\": \"one\"},\n    {\"name\":\"two\"}\n  ]\n}\n"
        );
    }
}
