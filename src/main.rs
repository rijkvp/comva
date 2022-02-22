use anyhow::{anyhow, Context, Result};
use clap::Parser;
use magick_rust::{magick_wand_genesis, MagickWand};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex, Once},
    thread,
};

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
struct Options {
    /// Compress image image files
    #[clap(short)]
    image_ext: Option<Option<String>>,

    /// Compress audio files
    #[clap(short)]
    audio_ext: Option<Option<String>>,

    /// Compress video files
    #[clap(short)]
    video_ext: Option<Option<String>>,

    /// Keep the original files, if original file is overwritten backup files are kept
    #[clap(short, long)]
    keep_files: bool,

    /// Image compression quality
    #[clap(short, long)]
    quality: Option<u16>,

    /// The amount of worker threads
    #[clap(short, long, default_value_t = 8)]
    threads: u64,
}

/// Type category of media
#[derive(Clone, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
pub enum MediaType {
    Image,
    Audio,
    Video,
}

// ImageQuick initialization
static IM_START: Once = Once::new();

fn main() -> Result<()> {
    let extensions: HashMap<&str, MediaType> = HashMap::from([
        ("gif", MediaType::Image),
        ("jpg", MediaType::Image),
        ("jpeg", MediaType::Image),
        ("png", MediaType::Image),
        ("bmp", MediaType::Image),
        ("webp", MediaType::Image),
        ("avif", MediaType::Image),
        ("mp4", MediaType::Video),
        ("avi", MediaType::Video),
        ("mov", MediaType::Video),
        ("flv", MediaType::Video),
        ("avi", MediaType::Video),
        ("mkv", MediaType::Video),
        ("mp3", MediaType::Audio),
        ("wav", MediaType::Audio),
        ("ogg", MediaType::Audio),
        ("flac", MediaType::Audio),
        ("opus", MediaType::Audio),
        ("m4a", MediaType::Audio),
        ("webm", MediaType::Audio),
    ]);

    let options = Options::parse();

    let dir = PathBuf::from("./")
        .canonicalize()
        .with_context(|| "Failed path canonicalization.")?;
    let index = index(&dir, extensions)?;

    IM_START.call_once(|| {
        magick_wand_genesis();
    });
    compress(index, &options)?;

    print!("Operation completed.");

    Ok(())
}

#[derive(Debug)]
pub struct MediaIndex {
    pub path: PathBuf,
    pub media_type: MediaType,
}

pub fn index(directory: &PathBuf, extensions: HashMap<&str, MediaType>) -> Result<Vec<MediaIndex>> {
    let mut index_items =
        index_files(directory, 0, &extensions).with_context(|| "Failed to index files")?;
    index_items.sort_by(|a, b| a.media_type.cmp(&b.media_type));
    Ok(index_items)
}

fn index_files(
    directory: &PathBuf,
    depth: u32,
    extensions: &HashMap<&str, MediaType>,
) -> Result<Vec<MediaIndex>> {
    let mut index = Vec::new();
    for file in fs::read_dir(directory)? {
        let path = file?.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy().to_string().to_lowercase();
                if let Some(media_type) = extensions.get(&ext_str as &str) {
                    index.push(MediaIndex {
                        path: path.clone(),
                        media_type: media_type.clone(),
                    });
                }
            }
        } else if path.is_dir() {
            let mut child_index = index_files(&path, depth + 1, extensions)?;
            index.append(&mut child_index);
        }
    }
    Ok(index)
}

fn compress(index: Vec<MediaIndex>, options: &Options) -> Result<()> {
    println!("Starting compression of {} files..", index.len());

    let pool = ThreadPool::new(options.threads as usize);
    for file in index.iter() {
        let mt = file.media_type.clone();
        let path = file.path.clone();
        let options = options.clone();
        pool.execute(move || {
            compress_file(mt, path, options);
        });
    }
    Ok(())
}

// Basic threadpool from the Rust book
pub struct ThreadPool {
    workers: Vec<Worker>,
    sender: mpsc::Sender<Message>,
}

type Job = Box<dyn FnOnce() + Send + 'static>;

enum Message {
    NewJob(Job),
    Terminate,
}

impl ThreadPool {
    pub fn new(size: usize) -> ThreadPool {
        assert!(size > 0);

        let (sender, receiver) = mpsc::channel();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut workers = Vec::with_capacity(size);

        for _ in 0..size {
            workers.push(Worker::new(Arc::clone(&receiver)));
        }

        ThreadPool { workers, sender }
    }

    pub fn execute<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let job = Box::new(f);

        self.sender.send(Message::NewJob(job)).unwrap();
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        for _ in &self.workers {
            self.sender.send(Message::Terminate).unwrap();
        }

        for worker in &mut self.workers {
            if let Some(thread) = worker.thread.take() {
                thread.join().unwrap();
            }
        }
    }
}

struct Worker {
    thread: Option<thread::JoinHandle<()>>,
}

impl Worker {
    fn new(receiver: Arc<Mutex<mpsc::Receiver<Message>>>) -> Worker {
        let thread = thread::spawn(move || loop {
            let message = receiver.lock().unwrap().recv().unwrap();

            match message {
                Message::NewJob(job) => {
                    job();
                }
                Message::Terminate => {
                    break;
                }
            }
        });

        Worker {
            thread: Some(thread),
        }
    }
}

fn compress_file(media_type: MediaType, source_path: PathBuf, options: Options) {
    let format_flag = match media_type {
        MediaType::Image => options.image_ext.clone(),
        MediaType::Audio => options.audio_ext.clone(),
        MediaType::Video => options.video_ext.clone(),
    };
    if let Some(ext_option) = format_flag {
        let output_ext = ext_option.unwrap_or({
            source_path
                .extension()
                .unwrap()
                .to_string_lossy()
                .to_string()
                .to_lowercase()
        });
        let output_path = source_path.with_extension(&output_ext);

        let mut input_path = source_path.clone();
        let mut overwritten = false;
        if source_path == output_path {
            let source_ext = source_path
                .extension()
                .unwrap()
                .to_string_lossy()
                .to_string();
            input_path.set_extension(source_ext + ".tmp");
            fs::rename(&source_path, &input_path).expect("Failed to rename input path.");
            overwritten = true;
        }

        if output_path.exists() {
            println!("Skiped {source_path:?}, output already exists!");
            return;
        }

        println!("Compressing {output_path:?}..");

        let result = match media_type {
            MediaType::Image => compress_image(&input_path, &output_path, &options),
            MediaType::Audio | MediaType::Video => {
                compress_ffmpeg(&input_path, &output_path, &output_ext)
            }
        };
        match result {
            Ok(_) => {
                if !options.keep_files {
                    fs::remove_file(&input_path).expect("Failed to remove file");
                } else if overwritten {
                    let backup_path = input_path.with_extension("backup");
                    fs::rename(&input_path, &backup_path)
                        .expect("Failed to rename input to backup.");
                }
            }
            Err(err) => eprintln!("Compression of {input_path:?} failed:\n{err}"),
        }
    }
}

fn compress_image(input_path: &PathBuf, output_path: &PathBuf, options: &Options) -> Result<()> {
    let input_str = input_path.to_string_lossy().to_string();
    let output_str = output_path.to_string_lossy().to_string();

    let mut wand = MagickWand::new();

    if let Some(quality) = options.quality {
        wand.set_compression_quality(quality as usize)
            .map_err(|_| anyhow!("Failed to set compression quality."))?;
    }

    wand.read_image(&input_str)
        .map_err(|_| anyhow!("Failed to read image."))?;
    wand.write_image(&output_str)
        .map_err(|_| anyhow!("Failed to write image."))?;
    Ok(())
}

fn compress_ffmpeg(input_path: &PathBuf, output_path: &PathBuf, output_ext: &str) -> Result<()> {
    let input_str = input_path.to_string_lossy().to_string();
    let output_str = output_path.to_string_lossy().to_string();

    // FFMPEG Settings
    // TODO: Find better settings
    let mut ffmpeg_settings = HashMap::from([
        // Audio Lossy
        ("mp3", vec!["-qscale:a", "2"]), // See: https://trac.ffmpeg.org/wiki/Encode/MP3
        // Audio Loseless
        ("flac", vec!["-compression_level", "12"]), // Max FLAC compression
        // Video Lossy
        // Use H.265 encoding with CRF 28
        // TODO: -r flag to limit framerate
        // TODO: Down scaling
        ("mp4", vec!["-vcodec", "libx265", "-crf", "28"]),
        ("mkv", vec!["-vcodec", "libx265", "-crf", "28"]),
        ("mov", vec!["-vcodec", "libx265", "-crf", "28"]),
        ("avi", vec!["-vcodec", "libx265", "-crf", "28"]),
    ]);

    let mut args = Vec::<&str>::new();
    if let Some(ffmpeg_args) = ffmpeg_settings.get_mut(&output_ext) {
        args.append(ffmpeg_args);
    }

    let output = Command::new("ffmpeg")
        .arg("-i")
        .arg(&input_str)
        .arg(&output_str)
        .args(args)
        .arg("-y") // Overwrite
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| "Failed to run command")?;
    if !output.status.success() {
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(format!(
            "Failed FFMPEG execution!\nStdErr: {stderr_str}\nStdOut: {stdout_str}"
        )));
    }
    Ok(())
}
