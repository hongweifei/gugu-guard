//! 分片上传会话：把大文件按块追加到目标同目录的临时文件，断线后按服务端记录的偏移续传。
//!
//! 会话只活在进程内：守护进程重启后未完成的上传作废，临时文件按前缀留在磁盘上，
//! 由 [`Uploads::sweep`] 在过期后清理。同一会话的分片串行写入，两个请求不会交错追加。

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;

use gugu_core::fs::TEMP_PREFIX;

/// 未完成会话的保留时长：超过这个时间没有任何分片进来就作废。
pub const SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// 新建会话的结果：编号与临时文件位置。
pub struct Started {
    pub id: String,
    pub temp: PathBuf,
}

/// 分片上传的会话表。
#[derive(Default)]
pub struct Uploads {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    counter: AtomicU64,
}

/// 一个未完成的上传。
pub struct Session {
    /// 完成后要落到的目标路径。
    pub path: PathBuf,
    /// 目标同目录下的临时文件；同盘改名才能原子就位。
    pub temp: PathBuf,
    /// 客户端声明的总字节数；0 表示不校验。
    pub size: u64,
    pub overwrite: bool,
    offset: Mutex<u64>,
    touched: Mutex<Instant>,
    /// 分片写入锁：读偏移、追加、推进偏移三步在同一把锁里完成。
    pub write: AsyncMutex<()>,
}

impl Session {
    #[must_use]
    pub fn offset(&self) -> u64 {
        *self
            .offset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_offset(&self, value: u64) {
        *self
            .offset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
    }

    fn touch(&self) {
        *self
            .touched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }

    fn idle(&self) -> Duration {
        self.touched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed()
    }
}

impl Uploads {
    /// 登记一个新会话，临时文件落在目标同目录，改名就位时不跨盘。
    pub fn begin(&self, path: PathBuf, size: u64, overwrite: bool) -> Started {
        let id = self.new_id();
        let temp = path
            .parent()
            .map(|parent| parent.join(format!("{TEMP_PREFIX}part-{id}")))
            .unwrap_or_else(|| PathBuf::from(format!("{TEMP_PREFIX}part-{id}")));
        let session = Arc::new(Session {
            path,
            temp: temp.clone(),
            size,
            overwrite,
            offset: Mutex::new(0),
            touched: Mutex::new(Instant::now()),
            write: AsyncMutex::new(()),
        });
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), session);
        Started { id, temp }
    }

    /// 取一个会话；访问即续期。
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()?;
        session.touch();
        Some(session)
    }

    /// 移出会话；完成与放弃都由调用方负责临时文件。
    pub fn take(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id)
    }

    /// 清掉过期的空闲会话，返回需要删除的临时文件。
    pub fn sweep(&self, ttl: Duration) -> Vec<PathBuf> {
        let mut store = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut expired = Vec::new();
        store.retain(|_, session| {
            if session.idle() > ttl {
                expired.push(session.temp.clone());
                return false;
            }
            true
        });
        expired
    }

    /// 编号：两轮随机种子哈希拼成 128 位十六进制，碰撞概率可以忽略。
    fn new_id(&self) -> String {
        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        let state = std::collections::hash_map::RandomState::new();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let first = state.hash_one(("gugu-upload", counter, stamp));
        let second = state.hash_one(("gugu-upload", stamp, counter ^ 0x9E37_79B9_7F4A_7C15));
        format!("{first:016x}{second:016x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sessions() -> Uploads {
        Uploads::default()
    }

    fn add(uploads: &Uploads, name: &str) -> String {
        uploads.begin(PathBuf::from(name), 10, false).id
    }

    #[test]
    fn ids_are_unique_and_long_enough() {
        let uploads = sessions();
        let first = add(&uploads, "a");
        let second = add(&uploads, "b");
        assert_eq!(first.len(), 32);
        assert_ne!(first, second);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn a_session_tracks_its_offset_and_leaves_on_take() {
        let uploads = sessions();
        let id = add(&uploads, "a");
        let session = uploads.get(&id).expect("会话应存在");
        assert_eq!(session.offset(), 0);
        session.set_offset(4);
        assert_eq!(uploads.get(&id).expect("仍存在").offset(), 4);

        assert!(uploads.take(&id).is_some());
        assert!(uploads.get(&id).is_none(), "取走后不再可见");
        assert!(uploads.take(&id).is_none());
    }

    #[test]
    fn sweep_drops_only_idle_sessions() {
        let uploads = sessions();
        let id = add(&uploads, "a");
        assert!(
            uploads.sweep(SESSION_TTL).is_empty(),
            "刚创建的会话不该被清"
        );
        std::thread::sleep(Duration::from_millis(10));
        let gone = uploads.sweep(Duration::from_millis(1));
        assert_eq!(gone.len(), 1);
        assert!(uploads.get(&id).is_none());
    }

    #[test]
    fn temp_files_sit_next_to_the_target() {
        let uploads = sessions();
        let started = uploads.begin(PathBuf::from("/data/app/report.txt"), 0, false);
        assert_eq!(
            started.temp.parent(),
            Some(std::path::Path::new("/data/app"))
        );
        assert!(started
            .temp
            .file_name()
            .expect("有文件名")
            .to_string_lossy()
            .starts_with(TEMP_PREFIX));
    }
}
