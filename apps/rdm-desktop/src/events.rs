//! 后台线程交给 GPUI 界面的事件。界面只在主线程里消费这份队列。

use rdm_bridge::BridgeCommand;
use rdm_domain::DownloadTask;
use rdm_resolver::ResolvedPost;

pub enum AppEvent {
    Progress {
        task: DownloadTask,
        speed: f64,
    },
    Bridge(BridgeCommand),
    MediaResolved {
        url: String,
        result: Result<ResolvedPost, String>,
    },
    Quit,
    ShutdownFinished,
    NewDownload,
}
