use std::path::PathBuf;

use clap::Parser;
use pdf::file::File;
use pdf_render::render_page;
use pdf_render::tracer::{DrawItem, TraceCache, Tracer};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    input: PathBuf,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(short, long)]
    page: Option<usize>,
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

    let mut res = String::new();
    for item in items {
        if let DrawItem::Text(txt) = item {
            res.push_str(&format!("{}\t{:?}\n", txt.text, txt.rect.0));
        }
    }

    if let Some(out_path) = args.output {
        std::fs::write(out_path, res).expect("failed to write to file");
    } else {
        println!("{}", res);
    }
}
