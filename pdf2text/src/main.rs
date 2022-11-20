use std::path::PathBuf;

use clap::Parser;
use pdf::file::File;
use pdf_render::tracer::{DrawItem, TraceCache, Tracer};
use pdf_render::{render_page, TextSpan};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    input: PathBuf,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(short, long)]
    page: Option<usize>,
}

fn items2text(items: &mut Vec<&TextSpan>) -> String {
    let factor = 5.;

    let norm_pos = |x: f32| (x * factor) as i32;

    let mut res = String::new();
    items.sort_by_key(|x| (x.rect.0[1] as i32, norm_pos(x.rect.0[0])));

    if items.is_empty() {
        return res;
    }

    let mut prev_y = 0.;
    let mut prev_x = 0.;
    for item in items.iter() {
        let x_diff = (norm_pos(item.rect.0[0]) - norm_pos(prev_x)) as f32 / factor;
        if !res.is_empty() {
            if x_diff < -10. || norm_pos(item.rect.0[1]) >= norm_pos(prev_y) {
                res += "\n";
            }
        }

        if !res.is_empty() && !res.ends_with("\n") {
            if x_diff > 0.1 {
                res += " ";
            }
        }

        res += &item.text;
        // res += "\t";
        // res += &format!(" ({:?}) ", item.rect);

        prev_x = item.rect.0[2];
        prev_y = item.rect.0[3];
    }

    res
}

fn main() {
    let args = Args::parse();

    let file = File::open(args.input).expect("failed to read PDF");
    let mut cache = TraceCache::new();
    let mut backend = Tracer::new(&mut cache);

    if let Some(page_i) = args.page {
        let page = file
            .pages()
            .nth(page_i)
            .expect(&format!("invalid page {}", page_i))
            .expect(&format!("invalid page {}", page_i));
        render_page(&mut backend, &file, &page, Default::default()).expect("failed to analyze PDF");
    } else {
        for (page_nr, page) in file.pages().enumerate() {
            let page = page.expect(&format!("invalid page {}", page_nr));
            eprintln!("=== PAGE {} ===\n", page_nr);
            render_page(&mut backend, &file, &page, Default::default())
                .expect("failed to analyze PDF");
        }
    }

    let items = backend.finish();
    let mut items: Vec<&TextSpan> = items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Text(text) => Some(text),
            _ => None,
        })
        .collect();

    let res = items2text(&mut items);

    if let Some(out_path) = args.output {
        std::fs::write(out_path, res).expect("failed to write to file");
    } else {
        println!("{}", res);
    }
}
