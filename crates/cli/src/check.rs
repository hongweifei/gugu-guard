//! `gugu check`：只校验配置文件，不启动、不修改任何东西。
//!
//! 覆盖：TOML 能否解析 → 每个进程字段是否合法 → 依赖是否存在环，
//! 并打印相对路径的解析基准（绿色部署体检）。

use anyhow::Result;
use gugu_core::config::AppConfig;
use std::path::Path;

pub(crate) fn run(config_path: &Path) -> Result<()> {
    let display_path = gugu_core::config::canonicalize_clean(config_path);
    println!("配置文件: {}", display_path.display());

    if !config_path.exists() {
        println!("  状态: 不存在");
        println!("  说明: 守护进程首次运行时会在此位置自动生成模板；本次按默认空配置校验。");
        return Ok(());
    }

    // 解析失败（TOML 语法错、类型不匹配）直接冒泡 → 非零退出
    let config = AppConfig::load(config_path)?;

    let config_dir = config_path
        .parent()
        .map(gugu_core::config::canonicalize_clean)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    println!("  相对路径基准: {}", config_dir.display());
    println!("  进程数: {}", config.processes.len());

    let mut problems = 0usize;

    let mut names: Vec<&String> = config.processes.keys().collect();
    names.sort();
    for name in names {
        let pc = &config.processes[name];
        match pc.validate() {
            Ok(()) => {
                let mut detail = pc.full_command();
                if let Some(dir) = &pc.working_dir {
                    detail.push_str(&format!("  [cwd: {}]", dir.display()));
                }
                println!("  ✓ {name}  {detail}");
            }
            Err(e) => {
                println!("  ✗ {name}  {e}");
                problems += 1;
            }
        }
    }

    match gugu_core::manager::topological_sort(&config.processes) {
        Ok(order) if !order.is_empty() => println!("  依赖顺序: {}", order.join(" → ")),
        Ok(_) => {}
        Err(e) => {
            println!("  ✗ 依赖: {e}");
            problems += 1;
        }
    }

    if problems == 0 {
        println!("结果: 配置有效");
        Ok(())
    } else {
        anyhow::bail!("配置校验失败: {problems} 处问题")
    }
}
