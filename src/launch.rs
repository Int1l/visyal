use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::io::Write;
use std::process::{Child, Command, Stdio};

use crate::Config;

pub fn log_path() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("BlackLauncher").join("game.log")
}

pub fn log_tail(n: usize) -> String {
    let s = std::fs::read_to_string(log_path()).unwrap_or_default();
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

fn set_opengl(dir: &Path) {
    let p = dir.join("options.txt");
    let s = std::fs::read_to_string(&p).unwrap_or_default();
    let mut found = false;
    let mut out: Vec<String> = s
        .lines()
        .map(|l| {
            if l.starts_with("graphicsApiPreference:") {
                found = true;
                "graphicsApiPreference:prefer_opengl".to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push("graphicsApiPreference:prefer_opengl".into());
    }
    let _ = std::fs::write(&p, out.join("\n") + "\n");
}

fn os_name() -> &'static str {
    if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "osx" } else { "linux" }
}

pub fn list_versions(game_dir: &str) -> Vec<String> {
    let mut v = vec![];
    if let Ok(rd) = std::fs::read_dir(Path::new(game_dir).join("versions")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().join(format!("{name}.json")).exists() {
                v.push(name);
            }
        }
    }
    v.sort();
    v.reverse();
    v
}

fn read_json(p: &Path) -> Result<Value, String> {
    let s = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| e.to_string())
}

fn rules_ok(rules: Option<&Value>) -> bool {
    let Some(arr) = rules.and_then(|r| r.as_array()) else { return true };
    let mut allowed = false;
    for r in arr {
        let os_ok = match r.get("os").and_then(|o| o.get("name")).and_then(|n| n.as_str()) {
            Some(n) => n == os_name(),
            None => true,
        };
        if os_ok && r.get("features").is_none() {
            allowed = r["action"] == "allow";
        }
    }
    allowed
}

struct Merged {
    libs: Vec<Value>,
    jvm: Vec<Value>,
    game: Vec<Value>,
    legacy_args: Option<String>,
    main_class: String,
    asset_index: String,
    jar_id: String,
}

fn load(dir: &Path, id: &str) -> Result<Merged, String> {
    let v = read_json(&dir.join("versions").join(id).join(format!("{id}.json")))?;
    let arr = |a: &str, b: &str| v[a][b].as_array().cloned().unwrap_or_default();
    let mut m = Merged {
        libs: v["libraries"].as_array().cloned().unwrap_or_default(),
        jvm: arr("arguments", "jvm"),
        game: arr("arguments", "game"),
        legacy_args: v["minecraftArguments"].as_str().map(String::from),
        main_class: v["mainClass"].as_str().unwrap_or("").into(),
        asset_index: v["assetIndex"]["id"].as_str().or(v["assets"].as_str()).unwrap_or("").into(),
        jar_id: id.into(),
    };
    if let Some(pid) = v["inheritsFrom"].as_str() {
        let p = load(dir, pid)?;
        m.libs.extend(p.libs);
        let mut j = p.jvm;
        j.extend(m.jvm);
        m.jvm = j;
        let mut g = p.game;
        g.extend(m.game);
        m.game = g;
        if m.legacy_args.is_none() { m.legacy_args = p.legacy_args; }
        if m.main_class.is_empty() { m.main_class = p.main_class; }
        if m.asset_index.is_empty() { m.asset_index = p.asset_index; }
        m.jar_id = p.jar_id;
    }
    Ok(m)
}

fn collect(args: &[Value]) -> Vec<String> {
    let mut out = vec![];
    for a in args {
        if let Some(s) = a.as_str() {
            out.push(s.to_string());
        } else if rules_ok(a.get("rules")) {
            match &a["value"] {
                Value::String(s) => out.push(s.clone()),
                Value::Array(x) => out.extend(x.iter().filter_map(|y| y.as_str().map(String::from))),
                _ => {}
            }
        }
    }
    out
}

fn sub(s: &str, vars: &[(&str, String)]) -> String {
    let mut o = s.to_string();
    for (k, v) in vars {
        o = o.replace(&format!("${{{k}}}"), v);
    }
    o
}

fn offline_uuid(name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h1);
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    ("offline", name).hash(&mut h2);
    format!("{:016x}{:016x}", h1.finish(), h2.finish())
}

pub fn launch(cfg: &Config) -> Result<Child, String> {
    if cfg.version.is_empty() {
        return Err("версия не выбрана".into());
    }
    let dir = PathBuf::from(&cfg.game_dir);
    if cfg.prefer_opengl {
        set_opengl(&dir);
    }
    let m = load(&dir, &cfg.version)?;

    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut seen = HashSet::new();
    let mut cp: Vec<String> = vec![];
    for lib in &m.libs {
        if !rules_ok(lib.get("rules")) { continue; }
        if lib.get("natives").is_some() && lib["downloads"]["artifact"].is_null() { continue; }
        let Some(name) = lib["name"].as_str() else { continue };
        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() < 3 { continue; }
        let key = format!("{}:{}:{}", parts[0], parts[1], parts.get(3).unwrap_or(&""));
        if !seen.insert(key) { continue; }
        let rel = match lib["downloads"]["artifact"]["path"].as_str() {
            Some(p) => p.to_string(),
            None => format!("{}/{a}/{v}/{a}-{v}.jar", parts[0].replace('.', "/"), a = parts[1], v = parts[2]),
        };
        let p = dir.join("libraries").join(rel);
        if p.exists() { cp.push(p.to_string_lossy().into()); }
    }
    let jar = dir.join("versions").join(&m.jar_id).join(format!("{}.jar", m.jar_id));
    cp.push(jar.to_string_lossy().into());
    let classpath = cp.join(sep);

    let natives = dir.join("versions").join(&cfg.version).join("natives");
    let vars: Vec<(&str, String)> = vec![
        ("auth_player_name", cfg.nickname.clone()),
        ("version_name", cfg.version.clone()),
        ("game_directory", cfg.game_dir.clone()),
        ("assets_root", dir.join("assets").to_string_lossy().into()),
        ("game_assets", dir.join("assets").to_string_lossy().into()),
        ("assets_index_name", m.asset_index.clone()),
        ("auth_uuid", offline_uuid(&cfg.nickname)),
        ("auth_access_token", "0".into()),
        ("auth_session", "0".into()),
        ("clientid", "0".into()),
        ("auth_xuid", "0".into()),
        ("user_type", "legacy".into()),
        ("user_properties", "{}".into()),
        ("version_type", "BlackLauncher".into()),
        ("natives_directory", natives.to_string_lossy().into()),
        ("launcher_name", "BlackLauncher".into()),
        ("launcher_version", "0.1".into()),
        ("classpath", classpath),
        ("classpath_separator", sep.into()),
        ("library_directory", dir.join("libraries").to_string_lossy().into()),
    ];

    let mut jvm: Vec<String> = if m.jvm.is_empty() {
        vec!["-Djava.library.path=${natives_directory}".into(), "-cp".into(), "${classpath}".into()]
    } else {
        collect(&m.jvm)
    };
    jvm = jvm.iter().map(|s| sub(s, &vars)).collect();
    if !jvm.iter().any(|a| a == "-cp" || a == "-classpath") {
        jvm.push("-cp".into());
        jvm.push(sub("${classpath}", &vars));
    }
    let game: Vec<String> = if !m.game.is_empty() {
        collect(&m.game)
    } else {
        m.legacy_args.clone().unwrap_or_default().split_whitespace().map(String::from).collect()
    }
    .iter()
    .map(|s| sub(s, &vars))
    .collect();

    let mut cmd = Command::new(&cfg.java_path);
    cmd.arg(format!("-Xmx{}M", cfg.ram_mb))
        .args(&jvm)
        .arg(&m.main_class)
        .args(&game)
        .current_dir(&dir);
    let lp = log_path();
    if let Some(d) = lp.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::File::create(&lp) {
        let _ = writeln!(f, "JAVA: {}\nARGS: -Xmx{}M {:?} {} {:?}\n", cfg.java_path, cfg.ram_mb, jvm, m.main_class, game);
        if let Ok(f2) = f.try_clone() {
            cmd.stdout(Stdio::from(f2));
        }
        cmd.stderr(Stdio::from(f));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.spawn().map_err(|e| format!("не удалось запустить Java ({}): {e}", cfg.java_path))
}
