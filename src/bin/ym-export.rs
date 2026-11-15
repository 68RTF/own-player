#[path = "../yandex_export.rs"]
mod yandex_export;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = yandex_export::run(&args) {
        eprintln!("Не удалось экспортировать плейлист: {error}");
        std::process::exit(1);
    }
}
