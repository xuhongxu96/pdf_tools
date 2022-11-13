extern crate pdf;
use log::warn;
use pdf::primitive::Name;

use std::collections::HashMap;
use std::convert::TryInto;
use std::rc::Rc;

use pdf::content::*;
use pdf::encoding::BaseEncoding;
use pdf::error::{PdfError, Result};
use pdf::font::*;
use pdf::object::*;
use pdf_encoding::{self, DifferenceForwardMap};

use euclid::Transform2D;

#[derive(Clone)]
enum Decoder {
    Map(DifferenceForwardMap),
    Cmap(ToUnicodeMap),
    None,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::None
    }
}

#[derive(Default, Clone)]
pub struct FontInfo {
    name: String,
    decoder: Decoder,
}

impl FontInfo {
    pub fn decode(&self, data: &[u8], out: &mut String) -> Result<()> {
        // println!("decoded by {}, data: {:?}", self.name, data);
        let is_gbk = self.name.contains("GBK");
        match &self.decoder {
            Decoder::Cmap(ref cmap) => {
                if data == &[16, 40] {
                    out.push('-');
                } else if is_gbk || data.starts_with(&[0xfe, 0xff]) {
                    // FIXME: really windows not chunks!?
                    for w in data.windows(2) {
                        let cp = u16::from_be_bytes(w.try_into().unwrap());
                        if let Some(s) = cmap.get(cp) {
                            out.push_str(s);
                        }
                    }
                } else {
                    out.extend(
                        data.iter()
                            .filter_map(|&b| cmap.get(b.into()).map(|v| v.to_owned())),
                    );
                }
                Ok(())
            }
            Decoder::Map(map) => {
                out.extend(
                    data.iter()
                        .filter_map(|&b| map.get(b).map(|v| v.to_owned())),
                );
                Ok(())
            }
            Decoder::None => {
                if data.starts_with(&[0xfe, 0xff]) {
                    utf16be_to_char(&data[2..]).try_for_each(|r| {
                        r.map_or(Err(PdfError::Utf16Decode), |c| {
                            out.push(c);
                            Ok(())
                        })
                    })
                } else if let Ok(text) = std::str::from_utf8(data) {
                    out.push_str(text);
                    Ok(())
                } else {
                    Err(PdfError::Utf16Decode)
                }
            }
        }
    }
}

struct FontCache<'src, T: Resolve> {
    fonts: HashMap<Name, Rc<FontInfo>>,
    page: &'src Page,
    resolve: &'src T,
    default_font: Rc<FontInfo>,
}

impl<'src, T: Resolve> FontCache<'src, T> {
    fn new(page: &'src Page, resolve: &'src T) -> Self {
        let mut cache = FontCache {
            fonts: HashMap::new(),
            page,
            resolve,
            default_font: Rc::new(FontInfo::default()),
        };

        cache.populate();

        cache
    }

    fn populate(&mut self) {
        if let Ok(resources) = self.page.resources() {
            for (name, font) in resources.fonts.iter() {
                if let Some(font) = font.as_ref() {
                    if let Ok(font) = self.resolve.get(font) {
                        self.add_font(name.clone(), font);
                    }
                }
            }

            for (font, _) in resources.graphics_states.values().filter_map(|gs| gs.font) {
                if let Ok(font) = self.resolve.get(font) {
                    if let Some(name) = &font.name {
                        self.add_font(name.clone(), font);
                    }
                }
            }
        }
    }

    fn add_font(&mut self, name: Name, font: RcRef<Font>) {
        let font_name = font.name.as_ref().unwrap().as_str();
        // println!("Adding font \"{}\"", name.as_str());
        let decoder = if let Some(to_unicode) = font.to_unicode(self.resolve) {
            let cmap = to_unicode.unwrap();
            Decoder::Cmap(cmap)
        } else if let Some(encoding) = font.encoding() {
            let map = match encoding.base {
                BaseEncoding::StandardEncoding => Some(&pdf_encoding::STANDARD),
                BaseEncoding::SymbolEncoding => Some(&pdf_encoding::SYMBOL),
                BaseEncoding::WinAnsiEncoding => Some(&pdf_encoding::WINANSI),
                BaseEncoding::MacRomanEncoding => Some(&pdf_encoding::MACROMAN),
                BaseEncoding::None => None,
                ref e => {
                    warn!("unsupported pdf encoding {:?}", e);
                    return;
                }
            };

            Decoder::Map(DifferenceForwardMap::new(
                map,
                encoding
                    .differences
                    .iter()
                    .map(|(k, v)| (*k, v.to_string()))
                    .collect(),
            ))
        } else {
            return;
        };

        self.fonts.insert(
            name,
            Rc::new(FontInfo {
                name: font_name.to_string(),
                decoder,
            }),
        );
    }

    fn get_by_font_name(&self, name: &Name) -> Rc<FontInfo> {
        self.fonts.get(name).unwrap_or(&self.default_font).clone()
    }

    fn get_by_graphic_state_name(&self, name: &str) -> Option<(Rc<FontInfo>, f32)> {
        self.page
            .resources()
            .ok()
            .and_then(|resources| resources.graphics_states.get(name))
            .and_then(|gs| gs.font)
            .map(|(font, font_size)| {
                let font = self
                    .resolve
                    .get(font)
                    .ok()
                    .map(|font| {
                        font.name
                            .as_ref()
                            .map(|name| self.get_by_font_name(name))
                            .unwrap_or_else(|| self.default_font.clone())
                    })
                    .unwrap_or_else(|| self.default_font.clone());

                (font, font_size)
            })
    }
}

#[derive(Clone, Default)]
pub struct TextState {
    pub font: Rc<FontInfo>,
    pub font_size: f32,
    pub text_leading: f32,
    pub matrix: Transform2D<f32, PdfSpace, PdfSpace>,
    pub text_matrix: Transform2D<f32, PdfSpace, PdfSpace>,
}

impl TextState {
    pub fn text_offset(&self) -> Point {
        Point {
            x: self.matrix.m31 + self.text_matrix.m31,
            y: self.matrix.m32 + self.text_matrix.m32,
        }
    }
}

pub fn ops_with_text_state<'src, T: Resolve>(
    page: &'src Page,
    resolve: &'src T,
) -> impl Iterator<Item = (Op, Rc<TextState>)> + 'src {
    page.contents.iter().flat_map(move |contents| {
        contents.operations(resolve).unwrap().into_iter().scan(
            (Rc::new(TextState::default()), FontCache::new(page, resolve)),
            |(state, font_cache), op| {
                let mut update_state = |update_fn: &dyn Fn(&mut TextState)| {
                    let old_state: &TextState = state;
                    let mut new_state = old_state.clone();

                    update_fn(&mut new_state);

                    *state = Rc::new(new_state);
                };

                match op {
                    Op::BeginText => {
                        update_state(&|state: &mut TextState| {
                            let old_matrix = state.matrix;
                            *state = Default::default();
                            state.matrix = old_matrix;
                        });
                    }
                    Op::GraphicsState { ref name } => {
                        update_state(&|state: &mut TextState| {
                            if let Some((font, font_size)) =
                                font_cache.get_by_graphic_state_name(name)
                            {
                                state.font = font;
                                state.font_size = font_size;
                            }
                        });
                    }
                    Op::TextFont { ref name, size } => {
                        update_state(&|state: &mut TextState| {
                            state.font = font_cache.get_by_font_name(name);
                            state.font_size = size;
                        });
                    }
                    Op::Leading { leading } => {
                        update_state(&|state: &mut TextState| state.text_leading = leading);
                    }
                    Op::TextNewline => {
                        update_state(&|state: &mut TextState| {
                            state.text_matrix = state.text_matrix.pre_translate(
                                Point {
                                    x: 0.0f32,
                                    y: state.text_leading,
                                }
                                .into(),
                            );
                        });
                    }
                    Op::MoveTextPosition { translation } => {
                        update_state(&|state: &mut TextState| {
                            state.text_matrix = state.text_matrix.pre_translate(translation.into());
                        });
                    }
                    Op::Transform { matrix } => {
                        update_state(&|state: &mut TextState| {
                            state.matrix = matrix.into();
                        });
                    }
                    Op::SetTextMatrix { matrix } => {
                        update_state(&|state: &mut TextState| {
                            state.text_matrix = matrix.into();
                        });
                    }
                    _ => {}
                }

                Some((op, state.clone()))
            },
        )
    })
}

pub fn page_text(page: &Page, resolve: &impl Resolve) -> Result<String, PdfError> {
    let x_inline_threshold = 50.;
    let x_threshold = 2.;
    let y_threshold = 2.;

    let mut out = String::new();
    let mut prev_offset = Point::default();
    for (op, text_state) in ops_with_text_state(page, resolve) {
        // println!("op: {:?}, {:?}", op, text_state.as_ref().matrix);
        let mut word = String::new();
        let x_factor = text_state.text_matrix.m11.abs();
        let y_factor = text_state.text_matrix.m22.abs();

        match op {
            Op::TextDraw { ref text } => {
                text_state.font.decode(&text.data, &mut word)?;
            }

            Op::TextDrawAdjusted { ref array } => {
                for data in array {
                    match data {
                        TextDrawAdjusted::Text(text) => {
                            text_state.font.decode(&text.data, &mut word)?;
                        }
                        &TextDrawAdjusted::Spacing(s) => {
                            if s.abs() > x_inline_threshold * x_factor {
                                word += " ";
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        if !word.is_empty() {
            let text_offset = text_state.text_offset();
            if (prev_offset.y - text_offset.y).abs() > y_factor * y_threshold
                && !out.is_empty()
                && !out.ends_with("\n")
            {
                out.push('\n');
            } else if (prev_offset.x - text_offset.x).abs() > x_factor * x_threshold
                && !out.is_empty()
                && !out.ends_with(' ')
            {
                if let Some(ch0) = out.chars().last() {
                    if let Some(ch1) = word.chars().next() {
                        if ch0.is_ascii() && ch1.is_ascii_alphanumeric() {
                            out.push(' ');
                        }
                    }
                }
            }

            // out.push_str(&format!(
            //     "({}, {}, diff: {})",
            //     prev_offset.x / x_factor,
            //     text_offset.x / x_factor,
            //     (prev_offset.x - text_offset.x).abs() / x_factor
            // ));
            out.push_str(&word);

            prev_offset = text_offset;
        }
    }
    Ok(out)
}
