//! Prompt templates. Kept as named constants rather than inline strings so they can be
//! reviewed and tuned in one place.
//!
//! These are deliberately more prescriptive than the sketch in the plan's §8. The bare
//! "Dịch đoạn text sau" instruction reliably produces word-for-word output that reads
//! like machine translation, and asking for `original`/`translated` pairs makes it worse
//! by nudging the model to translate sentence by sentence in isolation. The templates
//! below ask for idiomatic phrasing explicitly and tell the model to segment by
//! paragraph, so it has enough context to produce something a native speaker would
//! actually write.
//!
//! The text template also asks for the translation *only*, with no `original` echo. The
//! plan's schema had the model repeat the source text back, which roughly doubles the
//! generated tokens — and so the time to a result — for a value the app already has and
//! discards. Image mode keeps `original`, because there the source text is whatever the
//! model managed to read off the screen.

/// Low enough to stay faithful, high enough not to read like a dictionary lookup.
/// `f64`, not `f32`: 0.3f32 widens to 0.30000001192092896 in the JSON body.
pub const TEMPERATURE: f64 = 0.3;

/// Sent as the system turn ahead of either template.
pub const SYSTEM_PROMPT: &str = "Bạn là dịch giả chuyên nghiệp và là người bản xứ của ngôn ngữ đích. \
Bản dịch của bạn phải đọc như do người bản xứ viết ra một cách tự nhiên, KHÔNG được nghe như dịch máy \
và KHÔNG dịch từng từ. Luôn trả về JSON thuần, không kèm giải thích, không kèm quá trình suy luận, \
không dùng thẻ <think>.";

pub const TEXT_TEMPLATE: &str = r#"Dịch đoạn text sau sang {target_lang}.

Yêu cầu về chất lượng:
- Dịch thoát nghĩa và tự nhiên. Ưu tiên cách diễn đạt mà người bản xứ thực sự dùng, thay vì dịch sát từng chữ.
- Nếu dịch sát nghĩa nghe cứng hoặc lạ, hãy viết lại cho mượt miễn là giữ đúng ý.
- Giữ đúng ngữ điệu, sắc thái và mức độ trang trọng của bản gốc.
- Dùng thuật ngữ tự nhiên trong ngữ cảnh (kỹ thuật, hội thoại, văn bản trang trọng...).
- Giữ nguyên format: xuống dòng, danh sách, dấu câu, số liệu, đơn vị, mã nguồn.
- KHÔNG dịch tên riêng, tên thương hiệu, tên biến/hàm/lệnh, đường dẫn, URL.
- KHÔNG thêm giải thích, ghi chú, phiên âm, hay nội dung không có trong bản gốc.
- KHÔNG lặp lại bản gốc trong kết quả. Chỉ xuất bản dịch.

Cách tách segment:
- Tách theo ĐOẠN VĂN, không tách theo từng câu, để bản dịch mạch lạc.
- Nếu bản gốc chỉ là một câu hoặc một cụm từ, trả về đúng 1 segment.

Chỉ trả JSON: {"segments": [{"translated": "..."}]}

Text: {input_text}"#;

pub const IMAGE_TEMPLATE: &str = r#"Bạn là công cụ dịch màn hình. Nhìn vào ảnh được cung cấp và:
1. Đọc TOÀN BỘ đoạn text xuất hiện trong ảnh, theo đúng thứ tự đọc tự nhiên.
2. Dịch từng đoạn sang {target_lang}. Tự động nhận diện ngôn ngữ gốc.
3. Với mỗi đoạn, ước lượng vị trí trên ảnh theo % chiều rộng/cao (0-100), tính từ góc trên-trái.

Yêu cầu về chất lượng:
- Dịch thoát nghĩa và tự nhiên, đọc như người bản xứ viết, KHÔNG dịch từng từ.
- Giữ đúng ngữ điệu và mức độ trang trọng của bản gốc.
- Giữ nguyên format gốc: xuống dòng, danh sách, số liệu, đơn vị.
- KHÔNG dịch tên riêng, tên thương hiệu, mã, đường dẫn, URL.
- KHÔNG thêm giải thích hay nội dung không xuất hiện trong ảnh.
- Gộp các dòng thuộc cùng một đoạn/câu thành MỘT segment, đừng tách theo từng dòng hiển thị.
- Bỏ qua phần text bị cắt mất chữ đến mức không đọc được.

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
