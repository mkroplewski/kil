use crate::diagnostic::SourceSpan;

#[derive(Debug, Clone)]
pub struct SourceMap<'a> {
    source: &'a str,
}

impl<'a> SourceMap<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { source }
    }

    pub fn at_line_column(&self, line: usize, column: usize) -> SourceSpan {
        let mut offset = 0usize;
        for (idx, text) in self.source.split_inclusive('\n').enumerate() {
            if idx == line {
                offset += column.min(text.len());
                break;
            }
            offset += text.len();
        }
        SourceSpan {
            start: offset,
            end: (offset + 1).min(self.source.len()),
            line: line + 1,
            column: column + 1,
        }
    }

    /// Best-effort JSON source mapping. Syntax errors are exact; semantic paths
    /// resolve to the nearest matching key or scalar and always include line/column.
    pub fn span_for_path(&self, path: &str) -> Option<SourceSpan> {
        if path.is_empty() || path == "/" {
            return Some(self.at_offset(0, 1));
        }
        let mut cursor = 0usize;
        let mut best = None;
        for raw in path.split('/').skip(1) {
            if raw.is_empty() || raw.parse::<usize>().is_ok() {
                continue;
            }
            let segment = raw.replace("~1", "/").replace("~0", "~");
            let patterns = [format!("\"{segment}\""), segment];
            let haystack = &self.source[cursor..];
            let found = patterns
                .iter()
                .filter_map(|needle| haystack.find(needle).map(|offset| (offset, needle.len())))
                .min_by_key(|(offset, _)| *offset);
            let Some((relative, len)) = found else { break };
            let absolute = cursor + relative;
            best = Some(self.at_offset(absolute, len));
            cursor = absolute + len;
        }
        best
    }

    fn at_offset(&self, offset: usize, len: usize) -> SourceSpan {
        let prefix = &self.source[..offset.min(self.source.len())];
        let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
        let column = prefix
            .rsplit('\n')
            .next()
            .map_or(1, |s| s.chars().count() + 1);
        SourceSpan {
            start: offset,
            end: (offset + len.max(1)).min(self.source.len()),
            line,
            column,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_keys_resolve_inside_the_requested_parent() {
        let source = r#"{ "components": { "R1": { "symbol": "A" }, "R2": { "symbol": "B" } } }"#;
        let map = SourceMap::new(source);
        let span = map.span_for_path("/components/R2/symbol").unwrap();
        assert!(span.start > source.find("R2").unwrap());
        assert_eq!(&source[span.start..span.end], "\"symbol\"");
    }
}
