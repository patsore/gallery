use std::{env, fs};
use std::path::Path;
use std::sync::mpsc::channel;
use askama_axum::Template;
use axum::{
    routing::{get, post},
    http::StatusCode,
    Json, Router,
};
use axum::http::{Response, Uri};
use axum::response::{Html, IntoResponse};
use image::imageops::thumbnail;
use notify::{Event, EventKind, recommended_watcher, RecursiveMode, Watcher};
use notify::event::CreateKind;
use once_cell::sync::Lazy;
use tower_http::services::ServeDir;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

struct Config{
    static_folder: String,
    image_folder: String,
    thumbnail_folder: String,
}

static CONFIG: Lazy<Config> = Lazy::new(|| {
    dotenvy::dotenv().unwrap();
    Config {
        static_folder: env::var("STATIC_FOLDER").expect("STATIC_FOLDER must be set"),
        image_folder: env::var("IMAGE_FOLDER").expect("IMAGE_FOLDER must be set"),
        thumbnail_folder: env::var("THUMBNAIL_FOLDER").expect("THUMBNAIL_FOLDER must be set"),
    }
});


#[tokio::main]
async fn main(){
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    init_directories();
    
    let serve_dir = ServeDir::new(&CONFIG.static_folder);

    generate_startup_thumbnails();

    tokio::spawn(monitor_directory());

    tracing::info!("Initialized thumbnails and started monitoring directory");

    let app = Router::new()
        .route("/", get(root))
        .nest_service("/static", serve_dir);

    tracing::info!("Server started on port 3000");

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();

}

struct Image {
    original: String,
    thumbnail: String,
}

#[derive(Template)]
#[template(path = "gallery.html")]
struct GalleryTemplate {
    images: Vec<Image>,
}

async fn root() -> Html<String> {
    let dir = fs::read_dir(&CONFIG.image_folder).unwrap();
    let images = dir
        .filter_map(|entry| {
            entry.ok().and_then(|e| {
                let file_name = e.file_name().into_string().unwrap();
                let file_name_webp = Path::new(&file_name).with_extension("webp");
                let original = format!("static/images/{file_name}");
                let thumbnail = format!("static/thumbnails/{}", file_name_webp.display());
                Some(Image { original, thumbnail })
            })
        })
        .collect::<Vec<Image>>();

    let template = GalleryTemplate { images };
    Html(template.render().unwrap())
}

async fn monitor_directory() {
    let (tx, rx) = channel();
    let mut watcher = recommended_watcher(tx).unwrap();
    watcher.watch(Path::new(&CONFIG.image_folder), RecursiveMode::Recursive).unwrap();

    while let Ok(Ok(event)) = rx.recv() {
        if let EventKind::Create(CreateKind::File) = event.kind {
            if let Some(path) = event.paths.first() {
                let file_name = path.file_name().unwrap();
                let thumbnail_path = Path::new(&CONFIG.thumbnail_folder).join(file_name);
                create_thumbnail(path, &thumbnail_path);
            }
        }
    }
}

fn init_directories(){
    let static_folder = Path::new(&CONFIG.static_folder);
    let image_folder = Path::new(&CONFIG.image_folder);
    let thumbnail_folder = Path::new(&CONFIG.thumbnail_folder);

    if !static_folder.exists(){
        fs::create_dir_all(static_folder).unwrap();
    }

    if !image_folder.exists(){
        fs::create_dir_all(image_folder).unwrap();
    }

    if !thumbnail_folder.exists(){
        fs::create_dir_all(thumbnail_folder).unwrap();
    }
}

fn generate_startup_thumbnails(){
    let dir = fs::read_dir(&CONFIG.image_folder).unwrap();
    dir.for_each(|entry| {
        let entry = entry.unwrap();
        let file_name = entry.file_name();
        let thumbnail_path = Path::new(&CONFIG.thumbnail_folder).join(file_name).with_extension("webp");
        if !thumbnail_path.exists(){
            create_thumbnail(&entry.path(), &thumbnail_path);
        }
    });
}

fn create_thumbnail(image_path: &Path, output_path: &Path) {
    if let Ok(img) = image::open(image_path) {
        let thumb = img.resize(150, 150, image::imageops::FilterType::Lanczos3);
        let webp_path = output_path.with_extension("webp");
        thumb.save(webp_path).unwrap();
    }
}