use std::borrow::Cow;
use std::fmt::Write;

use comrak::{Options, markdown_to_html};
use html_escape::{decode_html_entities, encode_safe};
use maud::{Markup, PreEscaped, html};
use serde_json::Value;

use nixsearch_core::document::{DocText, DocValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CodeLanguage {
    Nix,
    Toml,
    Json,
    Bash,
    Fish,
    Ini,
    Yaml,
    Xml,
    Sql,
    Nushell,
    PlainText,
}

pub trait CodeHighlighter {
    fn highlight(&self, language: CodeLanguage, code: &str) -> Option<String>;
}

#[derive(Debug, Default)]
pub struct LumisHighlighter;

impl CodeHighlighter for LumisHighlighter {
    fn highlight(&self, language: CodeLanguage, code: &str) -> Option<String> {
        use lumis::formatters::Formatter;
        use lumis::{HtmlInlineBuilder, languages::Language, themes};

        let pre_class = format!("code-block {}", language_class(language));
        let language = match language {
            CodeLanguage::Nix => Language::Nix,
            CodeLanguage::Toml => Language::Toml,
            CodeLanguage::Json => Language::JSON,
            CodeLanguage::Bash => Language::Bash,
            CodeLanguage::Fish => Language::Fish,
            CodeLanguage::Ini => Language::INI,
            CodeLanguage::Yaml => Language::YAML,
            CodeLanguage::Xml => Language::XML,
            CodeLanguage::Sql => Language::SQL,
            CodeLanguage::Nushell => Language::Nushell,
            CodeLanguage::PlainText => Language::PlainText,
        };

        let theme = themes::get("onedark").ok()?;
        let formatter = HtmlInlineBuilder::new()
            .language(language)
            .theme(Some(theme))
            .pre_class(Some(pre_class))
            .build()
            .ok()?;
        let mut output = Vec::new();
        formatter.format(code, &mut output).ok()?;
        String::from_utf8(output).ok()
    }
}

pub fn render_doc_text(value: &DocText) -> Markup {
    match value {
        DocText::Markdown(value) => render_markdown(value),
        DocText::DocBook(value) => html! { p { (docbook_to_plain_text(value)) } },
        DocText::Plain(value) => html! { p { (value) } },
    }
}

pub fn render_doc_value(value: &DocValue) -> Markup {
    match value {
        DocValue::NixExpression(value) => render_code(CodeLanguage::Nix, &format_nix(value)),
        DocValue::Json(value) => render_code(CodeLanguage::Nix, &format_nix(&json_to_nix(value))),
        DocValue::Markdown(value) => render_markdown(value),
        DocValue::DocBook(value) => html! { p { (docbook_to_plain_text(value)) } },
        DocValue::Plain(value) => render_code(CodeLanguage::PlainText, value),
    }
}

pub fn render_code(language: CodeLanguage, code: &str) -> Markup {
    let highlighter = LumisHighlighter;
    if let Some(body) = highlighter.highlight(language, code) {
        return html! { (PreEscaped(body)) };
    }

    let body = encode_safe(code).into_owned();
    let language_class = language_class(language);

    html! {
        pre.code-block class=(language_class) { code { (PreEscaped(body)) } }
    }
}

fn language_class(language: CodeLanguage) -> &'static str {
    match language {
        CodeLanguage::Nix => "language-nix",
        CodeLanguage::Toml => "language-toml",
        CodeLanguage::Json => "language-json",
        CodeLanguage::Bash => "language-bash",
        CodeLanguage::Fish => "language-fish",
        CodeLanguage::Ini => "language-ini",
        CodeLanguage::Yaml => "language-yaml",
        CodeLanguage::Xml => "language-xml",
        CodeLanguage::Sql => "language-sql",
        CodeLanguage::Nushell => "language-nushell",
        CodeLanguage::PlainText => "language-plain-text",
    }
}

fn render_markdown(value: &str) -> Markup {
    let markdown = preprocess_nix_doc_roles(value);
    let markdown = render_fenced_code_blocks(&markdown);
    let mut options = Options::default();
    options.render.unsafe_ = true;
    let html = markdown_to_html(&markdown, &options);
    let html = ammonia::Builder::default()
        .add_tags([
            "code", "pre", "span", "table", "thead", "tbody", "tr", "th", "td",
        ])
        .add_generic_attributes(["class", "style"])
        .clean(&html)
        .to_string();

    html! { div.doc-content { (PreEscaped(html)) } }
}

fn render_fenced_code_blocks(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut lines = value.lines();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("```") {
            output.push_str(line);
            output.push('\n');
            continue;
        }

        let info = trimmed.trim_start_matches("```").trim();
        let mut code = String::new();
        for code_line in lines.by_ref() {
            if code_line.trim_start().starts_with("```") {
                break;
            }
            code.push_str(code_line);
            code.push('\n');
        }

        let language = language_from_info(info, &code);
        let formatted = format_code(language, &code);
        output.push_str(&render_code(language, &formatted).into_string());
        output.push('\n');
    }

    output
}

fn language_from_info(info: &str, code: &str) -> CodeLanguage {
    let info = info
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match info.as_str() {
        "nix" => CodeLanguage::Nix,
        "toml" => CodeLanguage::Toml,
        "json" => CodeLanguage::Json,
        "bash" | "sh" | "shell" | "console" | "shellsession" => CodeLanguage::Bash,
        "fish" => CodeLanguage::Fish,
        "ini" => CodeLanguage::Ini,
        "yaml" | "yml" => CodeLanguage::Yaml,
        "xml" => CodeLanguage::Xml,
        "sql" => CodeLanguage::Sql,
        "nushell" | "nu" => CodeLanguage::Nushell,
        "" if nixfmt_rs::format(code).is_ok() => CodeLanguage::Nix,
        "" if serde_json::from_str::<Value>(code).is_ok() => CodeLanguage::Json,
        _ => CodeLanguage::PlainText,
    }
}

fn format_code(language: CodeLanguage, code: &str) -> Cow<'_, str> {
    match language {
        CodeLanguage::Nix => Cow::Owned(format_nix(code)),
        CodeLanguage::Json => serde_json::from_str::<Value>(code)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .map(Cow::Owned)
            .unwrap_or(Cow::Borrowed(code)),
        _ => Cow::Borrowed(code.trim_end()),
    }
}

fn preprocess_nix_doc_roles(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut in_fence = false;

    for line in value.split_inclusive('\n') {
        let line_without_newline = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = line_without_newline.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            output.push_str(line);
            continue;
        }

        if in_fence {
            output.push_str(line);
            continue;
        }

        output.push_str(&preprocess_nix_doc_roles_line(line_without_newline));
        if line.ends_with('\n') {
            output.push('\n');
        }
    }

    output
}

fn preprocess_nix_doc_roles_line(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;

    while !rest.is_empty() {
        if rest.starts_with('`') {
            let tick_count = rest.bytes().take_while(|&byte| byte == b'`').count();
            let fence = &rest[..tick_count];
            let after_ticks = &rest[tick_count..];
            let Some(tick_end) = after_ticks.find(fence) else {
                output.push_str(rest);
                return output;
            };
            let code_end = tick_count + tick_end + tick_count;
            output.push_str(&rest[..code_end]);
            rest = &rest[code_end..];
            continue;
        }

        if let Some((text, remaining)) = nix_doc_role_at_start(rest) {
            output.push('`');
            output.push_str(text);
            output.push('`');
            rest = remaining;
            continue;
        }

        let ch = rest.chars().next().expect("rest is not empty");
        output.push(ch);
        rest = &rest[ch.len_utf8()..];
    }

    output
}

fn nix_doc_role_at_start(value: &str) -> Option<(&str, &str)> {
    let role_end = value.strip_prefix('{')?.find("}`")? + 1;
    let role = &value[1..role_end];
    if !matches!(
        role,
        "option" | "file" | "var" | "command" | "env" | "manpage"
    ) {
        return None;
    }

    let after_role = &value[role_end + 2..];
    let value_end = after_role.find('`')?;
    Some((&after_role[..value_end], &after_role[value_end + 1..]))
}

pub fn format_nix(value: &str) -> String {
    nixfmt_rs::format(value)
        .map(|value| value.trim_end_matches('\n').to_owned())
        .unwrap_or_else(|_| value.trim_end().to_owned())
}

fn json_to_nix(value: &Value) -> String {
    json_to_nix_indent(value, 0)
}

fn json_to_nix_indent(value: &Value, indent: usize) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => nix_string(value, indent),
        Value::Array(values) => nix_array(values, indent),
        Value::Object(values) => nix_attrset(values, indent),
    }
}

fn nix_array(values: &[Value], indent: usize) -> String {
    if values.is_empty() {
        return "[ ]".to_owned();
    }
    if values.len() == 1 {
        let value = json_to_nix_indent(&values[0], indent);
        if !value.contains('\n') {
            return format!("[ {value} ]");
        }
    }

    let next_indent = indent + 2;
    let mut output = String::from("[\n");
    for value in values {
        let _ = writeln!(
            output,
            "{space}{value}",
            space = " ".repeat(next_indent),
            value = json_to_nix_indent(value, next_indent)
        );
    }
    output.push_str(&" ".repeat(indent));
    output.push(']');
    output
}

fn nix_attrset(values: &serde_json::Map<String, Value>, indent: usize) -> String {
    if values.is_empty() {
        return "{ }".to_owned();
    }
    if values.len() == 1 {
        let (key, value) = values.iter().next().expect("single item exists");
        let value = json_to_nix_indent(value, indent);
        if !value.contains('\n') {
            return format!("{{ {} = {value}; }}", nix_attr_key(key));
        }
    }

    let next_indent = indent + 2;
    let mut output = String::from("{\n");
    for (key, value) in values {
        let _ = writeln!(
            output,
            "{space}{key} = {value};",
            space = " ".repeat(next_indent),
            key = nix_attr_key(key),
            value = json_to_nix_indent(value, next_indent)
        );
    }
    output.push_str(&" ".repeat(indent));
    output.push('}');
    output
}

fn nix_attr_key(value: &str) -> Cow<'_, str> {
    let valid = !value.is_empty()
        && !value.contains(['/', ' '])
        && !value.as_bytes()[0].is_ascii_digit()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '\''));
    if valid {
        Cow::Borrowed(value)
    } else {
        Cow::Owned(format!("{:?}", value))
    }
}

fn nix_string(value: &str, _indent: usize) -> String {
    serde_json::to_string(value)
        .expect("serializing a string cannot fail")
        .replace("${", r"\${")
}

fn docbook_to_plain_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(tag_start) = rest.find('<') {
        output.push_str(&decode_html_entities(&rest[..tag_start]));
        rest = &rest[tag_start..];

        let Some(tag_end) = rest.find('>') else {
            output.push_str(&decode_html_entities(rest));
            return collapse_whitespace(&output);
        };

        let tag = rest[1..tag_end].trim().trim_start_matches('/');
        if tag.starts_with("para")
            || tag.starts_with("simpara")
            || tag.starts_with("listitem")
            || tag.starts_with("itemizedlist")
            || tag.starts_with("orderedlist")
        {
            output.push(' ');
        }
        rest = &rest[tag_end + 1..];
    }

    output.push_str(&decode_html_entities(rest));
    collapse_whitespace(&output)
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_values_are_printed_as_nix() {
        assert_eq!(json_to_nix(&json!({})), "{ }");
        assert_eq!(
            json_to_nix(&json!({ "hello": "world" })),
            "{ hello = \"world\"; }"
        );
        assert_eq!(json_to_nix(&json!([1])), "[ 1 ]");
    }

    #[test]
    fn json_strings_are_escaped_for_nix() {
        assert_eq!(json_to_nix(&json!("${pkgs.hello}")), r#""\${pkgs.hello}""#);
        assert_eq!(json_to_nix(&json!("quote: \"")), r#""quote: \"""#);
        assert_eq!(json_to_nix(&json!("one\ntwo")), r#""one\ntwo""#);
        assert_eq!(
            json_to_nix(&json!({ "not valid": "x" })),
            r#"{ "not valid" = "x"; }"#
        );
    }

    #[test]
    fn nix_doc_roles_become_inline_code() {
        assert_eq!(
            preprocess_nix_doc_roles("Use {option}`services.nginx.enable` here."),
            "Use `services.nginx.enable` here."
        );
    }

    #[test]
    fn nix_doc_roles_inside_code_are_unchanged() {
        assert_eq!(
            preprocess_nix_doc_roles("``{option}`services.nginx.enable` ``"),
            "``{option}`services.nginx.enable` ``"
        );
        assert_eq!(
            preprocess_nix_doc_roles("```\n{option}`services.nginx.enable`\n```"),
            "```\n{option}`services.nginx.enable`\n```"
        );
    }

    #[test]
    fn malformed_nix_doc_roles_are_unchanged() {
        assert_eq!(
            preprocess_nix_doc_roles("Use {unknown}`value` and {option}`unterminated."),
            "Use {unknown}`value` and {option}`unterminated."
        );
    }

    #[test]
    fn docbook_renders_as_readable_plain_text() {
        let rendered = render_doc_text(&DocText::DocBook(
            "<para>Hello <literal>world</literal> &amp; friends</para>".to_owned(),
        ))
        .into_string();

        assert!(rendered.contains("Hello world &amp; friends"));
        assert!(!rendered.contains("<para>"));
        assert!(!rendered.contains("<literal>"));
    }

    #[test]
    fn docbook_value_does_not_render_executable_html() {
        let rendered = render_doc_value(&DocValue::DocBook(
            "<para>Hello<script>alert('no')</script></para>".to_owned(),
        ))
        .into_string();

        assert!(rendered.contains("Hello"));
        assert!(rendered.contains("alert"));
        assert!(!rendered.contains("<script>"));
    }

    #[test]
    fn literal_expression_renders_as_highlighted_nix_not_json_wrapper() {
        let value = DocValue::NixExpression(
            r#"{
  "browser.startup.homepage" = "https://nixos.org";
  "browser.search.isUS" = false;
}
"#
            .to_owned(),
        );

        let rendered = render_doc_value(&value).into_string();

        assert!(rendered.contains("browser.startup.homepage"));
        assert!(rendered.contains("https://nixos.org"));
        assert!(!rendered.contains("literalExpression"));
        assert!(!rendered.contains("_type"));
        assert!(!rendered.contains(r#"\n"#));
    }

    #[test]
    fn highlighted_code_does_not_render_nested_pre_blocks() {
        let rendered = render_code(CodeLanguage::Nix, "{ }").into_string();

        assert_eq!(rendered.matches("<pre").count(), 1);
        assert!(rendered.contains("code-block"));
        assert!(rendered.contains("language-nix"));
    }

    #[test]
    fn markdown_fences_are_formatted_and_highlighted() {
        let rendered = render_doc_text(&DocText::Markdown(
            "Example:\n\n```nix\n{foo=\"bar\";}\n```".to_owned(),
        ))
        .into_string();

        assert!(rendered.contains("code-block"));
        assert!(rendered.contains("language-nix"));
        assert!(rendered.contains("foo"));
        assert!(rendered.contains("bar"));
        assert!(!rendered.contains("```"));
    }

    #[test]
    fn markdown_output_is_sanitized() {
        let rendered = render_doc_text(&DocText::Markdown(
            "Safe <script>alert('no')</script> text".to_owned(),
        ))
        .into_string();

        assert!(rendered.contains("Safe"));
        assert!(rendered.contains("text"));
        assert!(!rendered.contains("<script"));
    }
}
