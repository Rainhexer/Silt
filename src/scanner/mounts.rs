//! Mount/filesystem enumeration via sysinfo.

use std::path::PathBuf;

use serde::Serialize;
use sysinfo::Disks;

/// What kind of storage backs a mount. Remote kinds (Cloud/Network) are
/// treated specially: scanning them can trigger downloads (rclone VFS,
/// sshfs, …) and their reported capacity is often synthetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MountKind {
    /// Physical/local block device.
    Local,
    /// Cloud-object storage mounted via FUSE (rclone, s3fs, gcsfuse, …).
    Cloud,
    /// Traditional network filesystem (NFS, SMB/CIFS, sshfs, WebDAV, …).
    Network,
}

impl MountKind {
    pub fn is_remote(self) -> bool {
        !matches!(self, MountKind::Local)
    }

    /// Short human label for UI badges.
    pub fn label(self) -> &'static str {
        match self {
            MountKind::Local => "local",
            MountKind::Cloud => "cloud",
            MountKind::Network => "network",
        }
    }
}

/// Classify a filesystem type string into a mount kind.
pub fn classify_fs(fs_type: &str) -> MountKind {
    let fs = fs_type.to_ascii_lowercase();
    // FUSE cloud-storage adapters. `fuse.rclone`, `s3fs`, `fuse.s3fs`, etc.
    const CLOUD: [&str; 8] = [
        "rclone", "s3fs", "gcsfuse", "blobfuse", "onedrive", "gdrive", "gdfs", "juicefs",
    ];
    if CLOUD.iter().any(|c| fs.contains(c)) {
        return MountKind::Cloud;
    }
    // Network filesystems: exact prefixes/names, not substrings, so e.g.
    // "tmpfs" never matches "fs".
    let net = matches!(
        fs.as_str(),
        "nfs" | "nfs4" | "cifs" | "smbfs" | "smb3" | "afpfs" | "ncpfs" | "9p" | "afs"
            | "glusterfs" | "cephfs" | "ceph" | "lustre" | "ocfs2" | "gfs2" | "davfs"
    ) || fs.starts_with("fuse.sshfs")
        || fs == "sshfs"
        || fs.starts_with("fuse.davfs")
        || fs.starts_with("fuse.curlftpfs")
        || fs.starts_with("fuse.cephfs")
        || fs.starts_with("fuse.glusterfs");
    if net {
        MountKind::Network
    } else {
        MountKind::Local
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MountInfo {
    pub device: String,
    pub mount_point: PathBuf,
    pub fs_type: String,
    pub kind: MountKind,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
}

impl MountInfo {
    pub fn used_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        self.used_bytes as f64 / self.total_bytes as f64
    }

    pub fn is_remote(&self) -> bool {
        self.kind.is_remote()
    }

    /// True when the reported capacity is a placeholder, not a real quota.
    /// rclone reports exactly 1 PiB when the remote has no known limit;
    /// treat anything that absurd on a remote mount as "unknown".
    pub fn total_is_synthetic(&self) -> bool {
        const PIB: u64 = 1 << 50;
        self.is_remote() && self.total_bytes >= PIB
    }
}

/// Enumerate real mounted filesystems, skipping pseudo-filesystems.
pub fn list_mounts() -> Vec<MountInfo> {
    let disks = Disks::new_with_refreshed_list();
    let mut mounts: Vec<MountInfo> = disks
        .iter()
        .filter_map(|disk| {
            let fs_type = disk.file_system().to_string_lossy().into_owned();
            // sysinfo already filters most pseudo-fs, but be defensive.
            if matches!(fs_type.as_str(), "tmpfs" | "devtmpfs" | "overlay" | "squashfs") {
                return None;
            }
            let total = disk.total_space();
            if total == 0 {
                return None;
            }
            let available = disk.available_space();
            Some(MountInfo {
                device: disk.name().to_string_lossy().into_owned(),
                mount_point: disk.mount_point().to_path_buf(),
                kind: classify_fs(&fs_type),
                fs_type,
                total_bytes: total,
                available_bytes: available,
                used_bytes: total.saturating_sub(available),
            })
        })
        .collect();

    mounts.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    mounts.dedup_by(|a, b| a.mount_point == b.mount_point);
    mounts
}
