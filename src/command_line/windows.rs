use anyhow::{Context, Result, bail};
use std::{collections::HashSet, fs, path::Path};

use crate::paths;

const MAX_RESPONSE_DEPTH: usize = 16;

pub fn tokenize(command_line: &str) -> Result<Vec<String>> {
    let mut arguments = Vec::new();
    let characters: Vec<char> = command_line.chars().collect();
    let mut index = 0;

    while index < characters.len() {
        while index < characters.len() && characters[index].is_whitespace() {
            index += 1;
        }
        if index == characters.len() {
            break;
        }

        let mut argument = String::new();
        let mut quoted = false;
        while index < characters.len() {
            let mut backslashes = 0;
            while index < characters.len() && characters[index] == '\\' {
                backslashes += 1;
                index += 1;
            }

            if index < characters.len() && characters[index] == '"' {
                argument.push_str(&"\\".repeat(backslashes / 2));
                if backslashes % 2 == 1 {
                    argument.push('"');
                    index += 1;
                } else if quoted && index + 1 < characters.len() && characters[index + 1] == '"' {
                    argument.push('"');
                    index += 2;
                } else {
                    quoted = !quoted;
                    index += 1;
                }
                continue;
            }

            argument.push_str(&"\\".repeat(backslashes));
            if index == characters.len() || (!quoted && characters[index].is_whitespace()) {
                break;
            }
            argument.push(characters[index]);
            index += 1;
        }

        if quoted {
            bail!("unterminated quote in compiler command line");
        }
        arguments.push(argument);
    }

    Ok(arguments)
}

pub fn expand_response_files(
    arguments: &[String],
    working_directory: &Path,
) -> Result<Vec<String>> {
    expand_response_files_inner(arguments, working_directory, &mut HashSet::new(), 0)
}

fn expand_response_files_inner(
    arguments: &[String],
    working_directory: &Path,
    active: &mut HashSet<String>,
    depth: usize,
) -> Result<Vec<String>> {
    if depth > MAX_RESPONSE_DEPTH {
        bail!("response-file nesting exceeds {MAX_RESPONSE_DEPTH} levels");
    }

    let mut expanded = Vec::new();
    for argument in arguments {
        let Some(path_text) = argument.strip_prefix('@') else {
            expanded.push(argument.clone());
            continue;
        };
        if path_text.is_empty() {
            expanded.push(argument.clone());
            continue;
        }

        let path =
            paths::absolute_lexical(Path::new(path_text.trim_matches('"')), working_directory);
        let key = paths::key(&path);
        if !active.insert(key.clone()) {
            bail!("response-file cycle detected at {}", path.display());
        }
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read response file {}", path.display()))?;
        let contents = decode_response_file(&bytes)
            .with_context(|| format!("failed to decode response file {}", path.display()))?;
        let nested = tokenize(&contents)?;
        expanded.extend(expand_response_files_inner(
            &nested,
            working_directory,
            active,
            depth + 1,
        )?);
        active.remove(&key);
    }
    Ok(expanded)
}

fn decode_response_file(bytes: &[u8]) -> Result<String> {
    if let Some(bytes) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(bytes.to_vec()).context("invalid UTF-8 response file");
    }
    if let Some(bytes) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if bytes.len() % 2 != 0 {
            bail!("odd byte count in UTF-16LE response file");
        }
        let words = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&words).context("invalid UTF-16LE response file");
    }
    if let Some(bytes) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        if bytes.len() % 2 != 0 {
            bail!("odd byte count in UTF-16BE response file");
        }
        let words = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&words).context("invalid UTF-16BE response file");
    }
    String::from_utf8(bytes.to_vec()).context("response file is not UTF-8 and has no UTF-16 BOM")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use tempfile::TempDir;

    fn quote(argument: &str) -> String {
        if !argument.is_empty()
            && !argument
                .chars()
                .any(|character| character.is_whitespace() || character == '"')
        {
            return argument.to_owned();
        }
        let mut output = String::from("\"");
        let mut backslashes = 0;
        for character in argument.chars() {
            match character {
                '\\' => backslashes += 1,
                '"' => {
                    output.push_str(&"\\".repeat(backslashes * 2 + 1));
                    output.push('"');
                    backslashes = 0;
                }
                _ => {
                    output.push_str(&"\\".repeat(backslashes));
                    backslashes = 0;
                    output.push(character);
                }
            }
        }
        output.push_str(&"\\".repeat(backslashes * 2));
        output.push('"');
        output
    }

    #[test]
    fn tokenizes_windows_backslashes_and_quotes() {
        assert_eq!(
            tokenize(r#"/I"C:\Program Files\inc" /DNAME=\"value\" "a b.cpp""#).unwrap(),
            vec![r"/IC:\Program Files\inc", r#"/DNAME="value""#, "a b.cpp"]
        );
    }

    #[test]
    fn rejects_unterminated_quotes() {
        assert!(tokenize(r#"/I"broken"#).is_err());
    }

    #[test]
    fn tokenizes_quoted_trailing_backslash_without_corrupting_prefixes() {
        assert_eq!(
            tokenize(r#"/Fo"C:\project\src\lib\obj\x64\\" /Gd"#).unwrap(),
            vec![r"/FoC:\project\src\lib\obj\x64\", "/Gd"]
        );
    }

    #[test]
    fn expands_nested_response_files_from_the_invocation_directory() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("inner.rsp"), r#"/DVALUE=1"#).unwrap();
        fs::write(
            temp.path().join("outer.rsp"),
            r#"@inner.rsp /I"with space""#,
        )
        .unwrap();
        let result = expand_response_files(&["@outer.rsp".to_owned()], temp.path()).unwrap();
        assert_eq!(result, vec!["/DVALUE=1", "/Iwith space"]);
    }

    #[test]
    fn rejects_response_file_cycles() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("a.rsp"), "@b.rsp").unwrap();
        fs::write(temp.path().join("b.rsp"), "@a.rsp").unwrap();
        assert!(expand_response_files(&["@a.rsp".to_owned()], temp.path()).is_err());
    }

    #[test]
    fn expands_utf16_response_files() {
        let temp = TempDir::new().unwrap();
        let mut bytes = vec![0xFF, 0xFE];
        for word in "/DUNICODE_VALUE=1".encode_utf16() {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        fs::write(temp.path().join("utf16.rsp"), bytes).unwrap();
        assert_eq!(
            expand_response_files(&["@utf16.rsp".to_owned()], temp.path()).unwrap(),
            vec!["/DUNICODE_VALUE=1"]
        );
    }

    proptest! {
        #[test]
        fn tokenization_round_trips_windows_arguments(
            arguments in prop::collection::vec(
                prop::collection::vec(
                    prop_oneof![
                        Just('a'),
                        Just('Z'),
                        Just('0'),
                        Just(' '),
                        Just('\\'),
                        Just('"'),
                        Just(':'),
                        Just('-'),
                    ],
                    0..24,
                )
                .prop_map(|characters| characters.into_iter().collect::<String>()),
                0..12,
            )
        ) {
            let command_line = arguments
                .iter()
                .map(|argument| quote(argument))
                .collect::<Vec<_>>()
                .join(" ");
            prop_assert_eq!(tokenize(&command_line).unwrap(), arguments);
        }
    }
}
