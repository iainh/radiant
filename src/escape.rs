macro_rules! safe_output {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }
    };
}

safe_output!(SafeHtml);
safe_output!(SafeXml);
safe_output!(SafeJsonString);

pub(crate) fn html(value: &str, output: &mut String) {
    let mut unescaped = 0;
    for (index, byte) in value.bytes().enumerate() {
        let escaped = match byte {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            b'\'' => "&#x27;",
            _ => continue,
        };
        output.push_str(&value[unescaped..index]);
        output.push_str(escaped);
        unescaped = index + 1;
    }
    if unescaped < value.len() {
        output.push_str(&value[unescaped..]);
    }
}

#[doc(hidden)]
pub mod private {
    use crate::{MediaType, SafeHtml, SafeJsonString, SafeXml};

    pub trait RenderValue {
        fn render_value(&self, media_type: MediaType, output: &mut String);
    }

    impl<T: RenderValue + ?Sized> RenderValue for &T {
        #[inline]
        fn render_value(&self, media_type: MediaType, output: &mut String) {
            T::render_value(self, media_type, output);
        }
    }

    impl RenderValue for str {
        #[inline]
        fn render_value(&self, media_type: MediaType, output: &mut String) {
            match media_type {
                MediaType::Html | MediaType::Xml => super::html(self, output),
                MediaType::Json => super::json(self, output),
                MediaType::Text => output.push_str(self),
            }
        }
    }

    impl RenderValue for String {
        #[inline]
        fn render_value(&self, media_type: MediaType, output: &mut String) {
            self.as_str().render_value(media_type, output);
        }
    }

    impl RenderValue for bool {
        #[inline]
        fn render_value(&self, _media_type: MediaType, output: &mut String) {
            output.push_str(if *self { "true" } else { "false" });
        }
    }

    impl RenderValue for char {
        #[inline]
        fn render_value(&self, media_type: MediaType, output: &mut String) {
            let mut encoded = [0; 4];
            self.encode_utf8(&mut encoded)
                .render_value(media_type, output);
        }
    }

    macro_rules! integer_values {
        ($($ty:ty),* $(,)?) => {$(
            impl RenderValue for $ty {
                #[inline]
                fn render_value(&self, _media_type: MediaType, output: &mut String) {
                    output.push_str(itoa::Buffer::new().format(*self));
                }
            }
        )*};
    }

    integer_values!(
        i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize
    );

    macro_rules! float_values {
        ($($ty:ty),* $(,)?) => {$(
            impl RenderValue for $ty {
                #[inline]
                fn render_value(&self, _media_type: MediaType, output: &mut String) {
                    let mut buffer = ryu::Buffer::new();
                    let formatted = buffer.format(*self);
                    output.push_str(formatted.strip_suffix(".0").unwrap_or(formatted));
                }
            }
        )*};
    }

    float_values!(f32, f64);

    impl RenderValue for SafeHtml {
        fn render_value(&self, media_type: MediaType, output: &mut String) {
            if media_type == MediaType::Html {
                output.push_str(self.as_str());
            } else {
                self.as_str().render_value(media_type, output);
            }
        }
    }

    impl RenderValue for SafeXml {
        fn render_value(&self, media_type: MediaType, output: &mut String) {
            if media_type == MediaType::Xml {
                output.push_str(self.as_str());
            } else {
                self.as_str().render_value(media_type, output);
            }
        }
    }

    impl RenderValue for SafeJsonString {
        fn render_value(&self, media_type: MediaType, output: &mut String) {
            if media_type == MediaType::Json {
                output.push_str(self.as_str());
            } else {
                self.as_str().render_value(media_type, output);
            }
        }
    }

    pub trait Truthy {
        fn is_truthy(&self) -> bool;
    }

    impl<T: Truthy + ?Sized> Truthy for &T {
        fn is_truthy(&self) -> bool {
            T::is_truthy(self)
        }
    }

    impl Truthy for bool {
        fn is_truthy(&self) -> bool {
            *self
        }
    }
    impl Truthy for str {
        fn is_truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl Truthy for String {
        fn is_truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<T> Truthy for [T] {
        fn is_truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<T> Truthy for Vec<T> {
        fn is_truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<T> Truthy for Option<T> {
        fn is_truthy(&self) -> bool {
            self.is_some()
        }
    }

    macro_rules! numeric_truth {
        ($($ty:ty),* $(,)?) => {$(
            impl Truthy for $ty { fn is_truthy(&self) -> bool { *self != 0 as $ty } }
        )*};
    }
    numeric_truth!(
        i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64
    );
}

pub(crate) fn json(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            c if c <= '\u{1f}' => {
                use std::fmt::Write;
                let _ = write!(output, "\\u{:04x}", c as u32);
            }
            other => output.push(other),
        }
    }
}
