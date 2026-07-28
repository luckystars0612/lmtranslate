//! Prompt templates. Kept as named constants rather than inline strings so they can be
//! reviewed and tuned in one place.

pub const TEMPERATURE: f32 = 0.2;

pub const TEXT_TEMPLATE: &str = r#"Dịch đoạn text sau sang {target_lang}. Giữ nguyên format, không dịch tên riêng.
Chỉ trả JSON: {"segments": [{"original": "...", "translated": "..."}]}

Text: {input_text}"#;

pub const IMAGE_TEMPLATE: &str = r#"Bạn là công cụ dịch màn hình. Nhìn vào ảnh được cung cấp và:
1. Đọc TOÀN BỘ đoạn text xuất hiện trong ảnh, theo đúng thứ tự đọc tự nhiên.
2. Dịch từng đoạn sang {target_lang}. Tự động nhận diện ngôn ngữ gốc.
3. Giữ nguyên format gốc (xuống dòng, danh sách, số liệu, tên riêng không dịch).
4. Với mỗi đoạn, ước lượng vị trí trên ảnh theo % chiều rộng/cao (0-100), tính từ góc trên-trái.

Chỉ trả về JSON thuần, theo schema:
{"segments": [{"original": "...", "translated": "...", "x_pct": 0-100, "y_pct": 0-100, "w_pct": 0-100}]}"#;

pub fn text_prompt(target_lang: &str, input_text: &str) -> String {
    TEXT_TEMPLATE
        .replace("{target_lang}", target_lang)
        .replace("{input_text}", input_text)
}

pub fn image_prompt(target_lang: &str) -> String {
    IMAGE_TEMPLATE.replace("{target_lang}", target_lang)
}
