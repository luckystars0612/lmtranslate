//! Prompt templates. Kept as named constants rather than inline strings so they can be
//! reviewed and tuned in one place.
//!
//! **Written in English, deliberately.** Instruction-following is measurably more
//! reliable in English across models, and — more importantly here — it keeps the
//! *instruction* language independent of the *target* language. When the instructions
//! were Vietnamese, every request carried an implicit second signal that Vietnamese was
//! the wanted output, which is wrong for the other fifteen languages the app offers.
//! The target language is injected as `{target_lang}`; its default is set in
//! `config::DEFAULT_TARGET_LANG` and is still Vietnamese.
//!
//! These are more prescriptive than a bare "translate this", which reliably produces
//! word-for-word output that reads like machine translation. Asking for
//! `original`/`translated` pairs made it worse still, by nudging the model to translate
//! sentence by sentence in isolation, so the templates below ask for idiomatic phrasing
//! explicitly and tell the model to segment by paragraph — enough context to write what
//! a native speaker would.
//!
//! The text template also asks for the translation *only*, with no echo of the source.
//! Having the model repeat the source back roughly doubles the generated tokens — and so
//! the time to a result — for a value the app already has and discards. Image mode keeps
//! `original`, because there the source text is whatever the model managed to read off
//! the screen, and is genuinely new information.

/// Low enough to stay faithful, high enough not to read like a dictionary lookup.
/// `f64`, not `f32`: 0.3f32 widens to 0.30000001192092896 in the JSON body.
pub const TEMPERATURE: f64 = 0.3;

/// Sent as the system turn ahead of either template.
pub const SYSTEM_PROMPT: &str = "You are a professional translator and a native speaker \
of the target language. Your translations must read as though a native speaker wrote \
them naturally — never like machine translation, and never word-for-word. Always reply \
with pure JSON: no preamble, no explanation, no commentary, no reasoning, and never a \
<think> block.";

pub const TEXT_TEMPLATE: &str = r#"Translate the text below into {target_lang}.

Quality requirements:
- Translate the meaning, not the words. Prefer the phrasing a native speaker would actually use over a literal rendering.
- If a literal translation sounds stiff or unnatural, rewrite it so that it flows, as long as the meaning is preserved.
- Preserve the register, tone and level of formality of the source.
- Use terminology that fits the context, whether that is technical, conversational, legal or marketing copy.
- Preserve formatting exactly: line breaks, lists, punctuation, numbers, units and source code.
- Do NOT translate proper nouns, brand names, variable/function/command names, file paths or URLs.
- Do NOT add explanations, notes, transliterations, or anything absent from the source.
- Do NOT repeat the source text in your answer. Output the translation only.

Segmentation:
- Split by PARAGRAPH, not by sentence, so the translation reads coherently.
- If the source is a single sentence or phrase, return exactly one segment.

Reply with JSON only: {"segments": [{"translated": "..."}]}

Text: {input_text}"#;

pub const IMAGE_TEMPLATE: &str = r#"You are a screen translation tool. Look at the supplied image and:
1. Read ALL text visible in the image, in natural reading order.
2. Translate each passage into {target_lang}. Detect the source language yourself.
3. For each passage, estimate its position in the image as a percentage of the width and height (0-100), measured from the top-left corner.

Quality requirements:
- Translate the meaning, not the words: the result must read as though a native speaker wrote it.
- Preserve the register, tone and level of formality of the source.
- Preserve the original formatting: line breaks, lists, numbers and units.
- Do NOT translate proper nouns, brand names, code, file paths or URLs.
- Do NOT add explanations or anything that does not appear in the image.
- Merge lines belonging to the same paragraph or sentence into ONE segment; do not split by displayed line.
- Skip any text cropped badly enough to be unreadable.

Reply with pure JSON only, in this schema:
{"segments": [{"original": "...", "translated": "...", "x_pct": 0-100, "y_pct": 0-100, "w_pct": 0-100}]}"#;

pub fn text_prompt(target_lang: &str, input_text: &str) -> String {
    TEXT_TEMPLATE
        .replace("{target_lang}", target_lang)
        .replace("{input_text}", input_text)
}

pub fn image_prompt(target_lang: &str) -> String {
    IMAGE_TEMPLATE.replace("{target_lang}", target_lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_language_is_substituted_everywhere() {
        let text = text_prompt("Japanese", "hello");
        assert!(text.contains("into Japanese"));
        assert!(text.contains("Text: hello"));
        assert!(!text.contains("{target_lang}"));
        assert!(!text.contains("{input_text}"));

        let image = image_prompt("Korean");
        assert!(image.contains("into Korean"));
        assert!(!image.contains("{target_lang}"));
    }

    /// The JSON schema in the template contains braces; substitution must not disturb it.
    #[test]
    fn the_schema_survives_substitution() {
        let text = text_prompt("Vietnamese", "x");
        assert!(text.contains(r#"{"segments": [{"translated": "..."}]}"#));
    }

    /// Text mode must not ask for the source text back — that is the token cost the
    /// app deliberately avoids paying.
    #[test]
    fn text_mode_does_not_request_an_original_echo() {
        let text = text_prompt("Vietnamese", "x");
        assert!(!text.contains(r#""original""#));
        // Image mode still needs it.
        assert!(image_prompt("Vietnamese").contains(r#""original""#));
    }
}
