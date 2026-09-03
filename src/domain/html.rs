//! The one HTML text escaper shared by every place that generates markup
//! from trusted data (highlighted code, media attributes).

/// Appends `value` with the five characters that could change markup
/// meaning written as entities.
pub fn push_escaped(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_markup_significant_character_becomes_an_entity() {
        let mut output = String::new();
        push_escaped(&mut output, "a<b>&\"c\"'d'");
        assert_eq!(output, "a&lt;b&gt;&amp;&quot;c&quot;&#39;d&#39;");
    }
}
