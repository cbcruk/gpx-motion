mod cli;
mod color;
mod config;
mod encode;
mod frames;
mod http;
mod hud;
mod labels;
mod map;
mod perspective;
mod render2d;
mod render3d;
mod sprites;
mod terrain;
mod tiles;
mod track;

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use cli::Args;
use config::Config;
use frames::{Job, Shot, Sink};

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Validate the options, then render; returns the summary line.
fn run(args: Args) -> Result<String> {
    let config = Config::try_from(args)?;
    if let Some(jobs) = config.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs.max(1))
            .build_global()
            .with_context(|| format!("could not configure {jobs} threads"))?;
    }
    let tracks = track::load_tracks(&config.inputs, config.elevation_smoothing)?;
    let shot = Shot::of(&config, &tracks)?;
    let mut map = map::map_source(&config)?;
    let labels = labels::label_style(config.labels.as_ref(), config.pixel_ratio, map.as_ref())?;
    let job = Job::new(&config, shot, tracks, labels);
    let mut sink = Sink::open(&config)?;
    if config.is_3d() {
        render3d::render(&job, map.as_mut(), &mut sink)?;
    } else {
        render2d::render(&job, map.as_mut(), &mut sink)?;
    }
    sink.finish()?;
    Ok(summary(&job))
}

/// The line printed after a successful run.
fn summary(job: &Job) -> String {
    let config = job.config;
    let tracks = job.tracks.len();
    let output = config.output.display();
    match job.shot {
        Shot::Still => {
            let view = if config.is_3d() { " in 3D" } else { "" };
            let points = job.points.len();
            format!("Rendered {tracks} track(s){view}, {points} points -> {output}")
        }
        Shot::Video { ms, frames } => {
            let view = if config.is_3d() { " (3D)" } else { "" };
            format!(
                "Encoded {frames} frames @ {}fps{view}, {tracks} track(s) -> {output} ({:.1}s)",
                config.fps,
                ms / 1000.0
            )
        }
    }
}
