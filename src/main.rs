use std::{env, fs};
use std::fs::FileType;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;

use askama_axum::Template;
use axum::{
    http::StatusCode,
    Json,
    Router, routing::{get, post},
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

struct Config {
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
async fn main() {
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

    generate_startup_thumbnails_for_dir(&Path::new(&CONFIG.image_folder));

    tokio::spawn(monitor_directory());

    tracing::info!("Initialized thumbnails and started monitoring directory");

    let app = Router::new()
        .route("/gallery/", get(root))
        .route("/gallery/{*path}", get(gallery))
        .nest_service("/static", serve_dir);

    tracing::info!("Server started on port 3000");

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

struct Image {
    original: String,
    thumbnail: String,
    name: String,
}

macro_rules! extract_file_name {
    ($path:expr) => {
        {
            let path = std::path::Path::new($path);
            path.file_name().and_then(|name| name.to_str()).unwrap_or("")
        }
    };
}

#[derive(Template)]
#[template(path = "gallery.html")]
struct GalleryTemplate {
    images: Vec<Image>,
}

async fn root() -> impl IntoResponse {
    let dir = fs::read_dir(&CONFIG.image_folder).unwrap();
    let mut images = dir
        .filter_map(|entry| {
            entry.ok().and_then(|e| {
                let file_type = e.file_type().unwrap();
                let file_name = e.file_name().into_string().unwrap();

                if file_type.is_file() {
                    let file_name_webp = Path::new(&file_name).with_extension("webp");
                    let original = format!("/static/images/{}", file_name);
                    let thumbnail = format!("/static/thumbnails/{}", file_name_webp.display());
                    Some(Image { original, thumbnail, name: file_name })
                } else if file_type.is_dir() {
                    let folder_path = format!("/gallery/{}", file_name);
                    let thumbnail = "/static/assets/folder.svg".to_string();
                    Some(Image { original: folder_path, thumbnail, name: file_name })
                } else {
                    None
                }
            })
        })
        .collect::<Vec<Image>>();

    let template = GalleryTemplate { images };
    Html(template.render().unwrap()).into_response()
}

async fn gallery(axum::extract::Path(path): axum::extract::Path<String>) -> impl IntoResponse {
    let dir = fs::read_dir(Path::new(&CONFIG.image_folder).join(&path)).unwrap();
    let mut images = dir
        .filter_map(|entry| {
            entry.ok().and_then(|e| {
                let file_type = e.file_type().unwrap();
                let file_name = e.file_name().into_string().unwrap();

                if file_type.is_file() {
                    let file_name_webp = Path::new(&file_name).with_extension("webp");
                    let original = format!("/static/images/{}/{}", path, file_name);
                    let thumbnail = format!("/static/thumbnails/{}", file_name_webp.display());
                    Some(Image { original, thumbnail, name: file_name })
                } else if file_type.is_dir() {
                    let folder_path = format!("/gallery/{}/{}", path, file_name);
                    let thumbnail = "/static/assets/folder.svg".to_string();
                    Some(Image { original: folder_path, thumbnail, name: file_name })
                } else {
                    None
                }
            })
        })
        .collect::<Vec<Image>>();

    let template = GalleryTemplate { images };
    Html(template.render().unwrap()).into_response()
}


async fn monitor_directory() {
    let (tx, rx) = channel();
    let mut watcher = recommended_watcher(tx).unwrap();
    watcher.watch(Path::new(&CONFIG.image_folder), RecursiveMode::Recursive).unwrap();

    while let Ok(Ok(event)) = rx.recv() {
        if let EventKind::Create(CreateKind::Any) = event.kind {
            if let Some(path) = event.paths.first() {
                let file_name = path.file_name().unwrap();
                let thumbnail_path = Path::new(&CONFIG.thumbnail_folder).join(file_name);
                tracing::info!("Creating thumbnail for newly found file {:?}", file_name);
                create_thumbnail(path, &thumbnail_path);
            }
        }
    }
}

fn init_directories() {
    let static_folder = Path::new(&CONFIG.static_folder);
    let image_folder = Path::new(&CONFIG.image_folder);
    let thumbnail_folder = Path::new(&CONFIG.thumbnail_folder);

    if !static_folder.exists() {
        fs::create_dir_all(static_folder).unwrap();
    }

    if !image_folder.exists() {
        fs::create_dir_all(image_folder).unwrap();
    }

    if !thumbnail_folder.exists() {
        fs::create_dir_all(thumbnail_folder).unwrap();
    }
}

fn generate_startup_thumbnails_for_dir(dir: &Path) {
    let dir = fs::read_dir(dir).unwrap();
    dir.for_each(|entry| {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let file_name = entry.file_name();
        let path = entry.path();
        if file_type.is_dir() {
            generate_startup_thumbnails_for_dir(&path);
        } else {
            let thumbnail_path = Path::new(&CONFIG.thumbnail_folder).join(file_name).with_extension("webp");
            if !thumbnail_path.exists() {
                create_thumbnail(&path, &thumbnail_path);
            }
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