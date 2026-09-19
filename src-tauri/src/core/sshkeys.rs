//! SSH 密钥管理（设置中心）：`~/.ssh` 下成对密钥的列出 / 生成 / 删除。
//!
//! 非 git 能力（ADR-001 约束的是 git 操作，不进 GitEngine）：生成走
//! ssh-key crate 纯 Rust（ed25519-dalek / rsa），不依赖外部 ssh-keygen
//! 是否在 PATH。文件布局与 OpenSSH 约定一致：`<name>` + `<name>.pub`，
//! 私钥 0600（unix）。带口令的私钥在认证时由 P7 askpass 桥按
//! `ssh:<路径>` 弹窗并可选记忆，无需额外接线。

use crate::core::error::AppError;
use serde::{Deserialize, Serialize};
use ssh_key::private::{KeypairData, RsaKeypair};
use ssh_key::{Algorithm, LineEnding, PrivateKey as SshPrivateKey, PublicKey as SshPublicKey};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// 生成算法（specta：`{ kind: "ed25519" } | { kind: "rsa"; bits: number }`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SshKeyAlgorithm {
    Ed25519,
    Rsa { bits: u32 },
}

/// 生成参数。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SshKeyGenerateRequest {
    pub algorithm: SshKeyAlgorithm,
    /// 公钥注释（追加在 `.pub` 行尾；空 = 不写注释）。
    pub comment: Option<String>,
    /// 私钥口令；空 / None = 不加密。
    pub passphrase: Option<String>,
    /// 文件名（相对 `~/.ssh`）；None / 空 = 按算法自动命名，占用则追加序号。
    pub file_name: Option<String>,
}

/// 密钥条目（列表页）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct SshKeyInfo {
    /// `.pub` 路径。
    pub public_path: String,
    /// 私钥路径；None = 目录里只有公钥（不能设为活动密钥）。
    pub private_path: Option<String>,
    pub algorithm: String,
    pub bits: u32,
    /// `SHA256:<base64>`。
    pub fingerprint: String,
    pub comment: String,
    /// 私钥是否用口令加密。
    pub encrypted: bool,
    /// 单行 openssh 公钥文本（复制给 Git 托管平台用）。
    pub public_key: String,
}

/// `~/.ssh` 目录（Windows 取 `USERPROFILE`，其余取 `HOME`——与 git/ssh 的
/// home 判定一致，GUI 进程里两者始终有值）。
pub fn ssh_dir() -> Result<PathBuf, AppError> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    let home =
        home.ok_or_else(|| AppError::io_with_detail("home dir not found", "resolve ~/.ssh"))?;
    Ok(PathBuf::from(home).join(".ssh"))
}

/// 列出目录下可识别的成对密钥（以 `*.pub` 为锚点，私钥可选）。
/// 目录不存在 = 空列表；解析失败的 `.pub` 跳过（半成品/外部文件）。
pub fn list_keys(dir: &Path) -> Result<Vec<SshKeyInfo>, AppError> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(AppError::io_with_detail(e.to_string(), "read ~/.ssh")),
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().map(|e| e != "pub").unwrap_or(true) || !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(pk) = SshPublicKey::from_openssh(&text) else {
            continue;
        };
        let private_path = if path.extension().map(|e| e == "pub").unwrap_or(false) {
            Some(path.with_extension(""))
        } else {
            None
        };
        let has_private = private_path.as_ref().is_some_and(|p| p.is_file());
        let encrypted = has_private
            && private_path
                .as_deref()
                .map(private_key_encrypted)
                .unwrap_or(false);
        let public_key = first_line(&text);
        out.push(SshKeyInfo {
            public_path: path.display().to_string(),
            private_path: has_private
                .then(|| private_path.map(|p| p.display().to_string()))
                .flatten(),
            algorithm: pk.algorithm().to_string(),
            bits: public_key_bits(&pk),
            fingerprint: pk.fingerprint(ssh_key::HashAlg::Sha256).to_string(),
            comment: pk.comment().to_string(),
            encrypted,
            public_key,
        });
    }
    out.sort_by(|a, b| a.public_path.cmp(&b.public_path));
    Ok(out)
}

/// 生成密钥对并写入 `dir`（`<name>` + `<name>.pub`），返回条目。
pub fn generate_key(dir: &Path, req: &SshKeyGenerateRequest) -> Result<SshKeyInfo, AppError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::io_with_detail(e.to_string(), "create ~/.ssh"))?;
    ensure_private_dir_mode(dir);

    let comment = req.comment.as_deref().unwrap_or("").trim().to_string();
    let mut key = match req.algorithm {
        SshKeyAlgorithm::Ed25519 => {
            let mut rng = rand::rngs::OsRng;
            SshPrivateKey::random(&mut rng, Algorithm::Ed25519)
        }
        SshKeyAlgorithm::Rsa { bits } => {
            let bits = usize::try_from(bits)
                .ok()
                .filter(|b| (1024..=16384).contains(b))
                .ok_or_else(|| AppError::parse("RSA bits out of range (1024..=16384)"))?;
            let mut rng = rand::rngs::OsRng;
            let kp = RsaKeypair::random(&mut rng, bits)
                .map_err(|e| AppError::io_with_detail(e.to_string(), "rsa keygen"))?;
            SshPrivateKey::new(KeypairData::from(kp), comment.clone())
        }
    }
    .map_err(|e| AppError::io_with_detail(e.to_string(), "ssh keygen"))?;
    key.set_comment(comment.clone());

    let passphrase = req.passphrase.as_deref().unwrap_or("").to_string();
    if !passphrase.is_empty() {
        let mut rng = rand::rngs::OsRng;
        key = key
            .encrypt(&mut rng, &passphrase)
            .map_err(|e| AppError::io_with_detail(e.to_string(), "encrypt private key"))?;
    }

    let name = pick_file_name(dir, req)?;
    let private_path = dir.join(&name);
    let public_path = dir.join(format!("{name}.pub"));

    let private_text = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| AppError::io_with_detail(e.to_string(), "encode private key"))?;
    std::fs::write(&private_path, private_text.as_bytes())
        .map_err(|e| AppError::io_with_detail(e.to_string(), "write private key"))?;
    set_private_mode(&private_path);

    let pk = key.public_key();
    let public_text = pk
        .to_openssh()
        .map_err(|e| AppError::io_with_detail(e.to_string(), "encode public key"))?;
    std::fs::write(&public_path, format!("{public_text}\n"))
        .map_err(|e| AppError::io_with_detail(e.to_string(), "write public key"))?;

    tracing::info!(name = %name, algorithm = %pk.algorithm(), "ssh key generated");
    Ok(SshKeyInfo {
        public_path: public_path.display().to_string(),
        private_path: Some(private_path.display().to_string()),
        algorithm: pk.algorithm().to_string(),
        bits: public_key_bits(pk),
        fingerprint: pk.fingerprint(ssh_key::HashAlg::Sha256).to_string(),
        comment,
        encrypted: !passphrase.is_empty(),
        public_key: public_text,
    })
}

/// 删除密钥（私钥 + `.pub`，存在哪个删哪个）。`path` 接受私钥或 `.pub`
/// 路径；两者都必须位于 `dir` 内（防误删外部文件）。
pub fn delete_key(dir: &Path, path: &str) -> Result<(), AppError> {
    let target = PathBuf::from(path);
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| AppError::parse(format!("invalid key path: {path}")))?;
    // 规范化比较（macOS /var → /private/var 等符号链接场景）。
    let dir_canon = dir
        .canonicalize()
        .map_err(|e| AppError::io_with_detail(e.to_string(), "resolve ~/.ssh"))?;
    let parent_canon = parent
        .canonicalize()
        .map_err(|e| AppError::io_with_detail(e.to_string(), "resolve key dir"))?;
    if parent_canon != dir_canon {
        return Err(AppError::parse(format!(
            "refusing to delete outside {dir_canon:?}: {path}"
        )));
    }

    let is_pub = target.extension().map(|e| e == "pub").unwrap_or(false);
    let (private, public) = if is_pub {
        (target.with_extension(""), target.clone())
    } else {
        (target.clone(), sibling_pub(&target))
    };

    let removed_private = private.is_file() && std::fs::remove_file(&private).is_ok();
    let removed_public = public.is_file() && std::fs::remove_file(&public).is_ok();
    if !removed_private && !removed_public {
        return Err(AppError::io(format!("key not found: {path}")));
    }
    tracing::info!(path = %path, "ssh key deleted");
    Ok(())
}

// =====================
// 内部
// =====================

/// 文件名占用则追加 `_2`、`_3`…（默认名冲突的确定性消解）。
fn pick_file_name(dir: &Path, req: &SshKeyGenerateRequest) -> Result<String, AppError> {
    let base = req
        .file_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || match req.algorithm {
                SshKeyAlgorithm::Ed25519 => "id_ed25519".to_string(),
                SshKeyAlgorithm::Rsa { .. } => "id_rsa".to_string(),
            },
            |s| s.to_string(),
        );
    if base == "." || base == ".." || base.contains('/') || base.contains('\\') {
        return Err(AppError::parse(format!("invalid key file name: {base}")));
    }
    if !dir.join(&base).exists() && !dir.join(format!("{base}.pub")).exists() {
        return Ok(base);
    }
    for n in 2..=9999 {
        let candidate = format!("{base}_{n}");
        if !dir.join(&candidate).exists() && !dir.join(format!("{candidate}.pub")).exists() {
            return Ok(candidate);
        }
    }
    Err(AppError::parse("no free key file name"))
}

/// `id_x` → `id_x.pub`（无扩展名场景统一拼 `.pub`）。
fn sibling_pub(private: &Path) -> PathBuf {
    let mut s = private.as_os_str().to_os_string();
    s.push(".pub");
    PathBuf::from(s)
}

/// 私钥是否带口令（加密）。解析失败按未加密处理（损坏文件如实展示）。
fn private_key_encrypted(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| SshPrivateKey::from_openssh(&text).ok())
        .is_some_and(|pk| pk.is_encrypted())
}

/// `.pub` 文本首行（保留原样注释，供复制）。
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// 展示用位数：RSA 取模长，Ed25519 固定 256，其余按曲线/未知回落 0。
fn public_key_bits(pk: &SshPublicKey) -> u32 {
    match pk.key_data() {
        ssh_key::public::KeyData::Rsa(rsa) => {
            let bytes = rsa.n.as_positive_bytes().unwrap_or(&[]);
            let lead = bytes.first().map_or(0, |b| b.leading_zeros() as usize);
            (bytes.len() * 8 - lead) as u32
        }
        ssh_key::public::KeyData::Ed25519(_) => 256,
        ssh_key::public::KeyData::Ecdsa(ecdsa) => match ecdsa {
            ssh_key::public::EcdsaPublicKey::NistP256(_) => 256,
            ssh_key::public::EcdsaPublicKey::NistP384(_) => 384,
            ssh_key::public::EcdsaPublicKey::NistP521(_) => 521,
        },
        _ => 0,
    }
}

/// 新建目录时收紧权限（ssh 拒绝过宽的目录）；已存在目录不动用户配置。
#[cfg(unix)]
fn ensure_private_dir_mode(dir: &Path) {
    if dir.exists() {
        return;
    }
    if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
        tracing::warn!(error = %e, "chmod ~/.ssh failed");
    }
}

#[cfg(not(unix))]
fn ensure_private_dir_mode(_dir: &Path) {}

/// 私钥 0600（unix）；Windows 依赖配置文件目录的默认 ACL，不做处理。
#[cfg(unix)]
fn set_private_mode(path: &Path) {
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(error = %e, "chmod private key failed");
    }
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ibexgit-sshkeys-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn generate_ed25519_list_and_delete_roundtrip() {
        let dir = temp_dir("ed25519");
        let req = SshKeyGenerateRequest {
            algorithm: SshKeyAlgorithm::Ed25519,
            comment: Some("test@example.com".into()),
            passphrase: None,
            file_name: None,
        };
        let info = generate_key(&dir, &req).unwrap();
        assert_eq!(info.algorithm, "ssh-ed25519");
        assert_eq!(info.bits, 256);
        assert_eq!(info.comment, "test@example.com");
        assert!(info.fingerprint.starts_with("SHA256:"));
        assert!(info.public_key.starts_with("ssh-ed25519 "));
        assert!(!info.encrypted);
        assert!(dir.join("id_ed25519").is_file());
        assert!(dir.join("id_ed25519.pub").is_file());
        // unix 私钥权限收紧（CI 三平台只有 unix 有 mode 语义）。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("id_ed25519"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let listed = list_keys(&dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], info);
        assert!(listed[0].private_path.is_some());

        delete_key(&dir, &info.private_path.unwrap()).unwrap();
        assert!(!dir.join("id_ed25519").exists());
        assert!(!dir.join("id_ed25519.pub").exists());
        assert!(list_keys(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_encrypted_marks_flag_and_delete_via_pub_path() {
        let dir = temp_dir("enc");
        let req = SshKeyGenerateRequest {
            algorithm: SshKeyAlgorithm::Ed25519,
            comment: None,
            passphrase: Some("secret-pass".into()),
            file_name: Some("work".into()),
        };
        let info = generate_key(&dir, &req).unwrap();
        assert!(info.encrypted);
        // OpenSSH 格式私钥头 + 加密标记可被解析识别。
        let text = std::fs::read_to_string(dir.join("work")).unwrap();
        let pk = SshPrivateKey::from_openssh(&text).unwrap();
        assert!(pk.is_encrypted());

        // 用 .pub 路径删除同样命中成对文件。
        delete_key(&dir, &info.public_path).unwrap();
        assert!(!dir.join("work").exists());
        assert!(!dir.join("work.pub").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_collision_appends_index() {
        let dir = temp_dir("collision");
        let req = || SshKeyGenerateRequest {
            algorithm: SshKeyAlgorithm::Ed25519,
            comment: None,
            passphrase: None,
            file_name: Some("team".into()),
        };
        let first = generate_key(&dir, &req()).unwrap();
        let second = generate_key(&dir, &req()).unwrap();
        assert!(first.public_path.ends_with("team.pub"));
        assert!(second.public_path.ends_with("team_2.pub"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reject_path_traversal_and_outside_dir() {
        let dir = temp_dir("traversal");
        let err = generate_key(
            &dir,
            &SshKeyGenerateRequest {
                algorithm: SshKeyAlgorithm::Ed25519,
                comment: None,
                passphrase: None,
                file_name: Some("../evil".into()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid key file name"));

        // 目录里放一个真实密钥后，删除 dir 外路径必须被拒。
        let info = generate_key(
            &dir,
            &SshKeyGenerateRequest {
                algorithm: SshKeyAlgorithm::Ed25519,
                comment: None,
                passphrase: None,
                file_name: None,
            },
        )
        .unwrap();
        let outside =
            std::env::temp_dir().join(format!("ibexgit-outside-{}.pub", std::process::id()));
        std::fs::write(&outside, "ssh-ed25519 decoy\n").unwrap();
        assert!(delete_key(&dir, &outside.display().to_string()).is_err());
        assert!(outside.is_file());
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(info.bits == 256);
    }

    #[test]
    fn list_skips_unparseable_and_reports_pub_only() {
        let dir = temp_dir("pubonly");
        std::fs::write(dir.join("notes.txt"), "not a key").unwrap();
        std::fs::write(dir.join("broken.pub"), "garbage").unwrap();
        std::fs::write(
            dir.join("only.pub"),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHXPLw3QZ cries/this/is/not/a/valid/key-blob",
        )
        .unwrap();
        // 上面是随手串（解析必失败），真公钥用生成器产出一对再删私钥模拟。
        let info = generate_key(
            &dir,
            &SshKeyGenerateRequest {
                algorithm: SshKeyAlgorithm::Ed25519,
                comment: Some("pub-only".into()),
                passphrase: None,
                file_name: Some("real".into()),
            },
        )
        .unwrap();
        std::fs::remove_file(dir.join("real")).unwrap();

        let listed = list_keys(&dir).unwrap();
        assert_eq!(listed.len(), 1, "broken.pub / 非密钥文件应被跳过");
        assert_eq!(listed[0].public_path, info.public_path);
        assert!(listed[0].private_path.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_ssh_dir_is_empty_not_error() {
        let dir = temp_dir("missing");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(list_keys(&dir).unwrap().is_empty());
    }

    /// RSA 生成路径（2048 位控制测试时长；UI 选项为 4096）。
    #[test]
    fn generate_rsa_works() {
        let dir = temp_dir("rsa");
        let info = generate_key(
            &dir,
            &SshKeyGenerateRequest {
                algorithm: SshKeyAlgorithm::Rsa { bits: 2048 },
                comment: None,
                passphrase: None,
                file_name: None,
            },
        )
        .unwrap();
        assert_eq!(info.algorithm, "ssh-rsa");
        assert_eq!(info.bits, 2048);
        assert!(info.public_key.starts_with("ssh-rsa "));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
