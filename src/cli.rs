use clap::Parser;

use crate::config::{get_config_dir, get_data_dir};

#[derive(Parser, Debug)]
#[command(author, version = version(), about)]
pub struct Cli {
    /// Tick rate, i.e. number of ticks per second
    #[arg(short, long, value_name = "TPS", default_value_t = 4.0)]
    pub tick_rate: f64,

    /// Frame rate, i.e. number of frames per second
    #[arg(short, long, value_name = "FPS", default_value_t = 30.0)]
    pub frame_rate: f64,

    /// Domain name of target Lore instance
    #[arg(short, long, value_name = "lore-domain", default_value_t = String::from("lore.kernel.org"))]
    pub domain: String,

    /// Target mailing list
    #[arg(short, long, value_name = "mailing-list", default_value_t = String::from("amd-gfx"))]
    pub list: String,

    /// Xapian queries
    #[arg(short, long, value_name = "query", default_value_t = String::from("((s:patch OR s:rfc) AND NOT s:re:) AND rt:1.month.ago.."))]
    pub query: String,

    /// Path to local kernel tree git repository
    #[arg(long, value_name = "path")]
    pub kernel_tree: Option<String>,
}

const VERSION_MESSAGE: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " -",
    env!("VERGEN_GIT_DESCRIBE"),
    " (",
    env!("VERGEN_BUILD_DATE"),
    ")"
);

pub fn version() -> String {
    let author = clap::crate_authors!();

    // let current_exe_path = PathBuf::from(clap::crate_name!()).display().to_string();
    let config_dir_path = get_config_dir().display().to_string();
    let data_dir_path = get_data_dir().display().to_string();

    format!(
        "\
{VERSION_MESSAGE}

Authors: {author}

Config directory: {config_dir_path}
Data directory: {data_dir_path}"
    )
}
