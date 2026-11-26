use crate::cgroup::{format_bytes, slug, CgroupSpec, CpuLimit};

// ── CPU ────────────────────────────────────────────────────

mod cpu {
    use super::*;

    #[test]
    fn parses_percent_of_one_core() {
        let limit = CpuLimit::parse("50%").unwrap().unwrap();
        assert_eq!(limit.quota, 50_000, "50% 单核 = 50000µs / 100000µs");
        assert_eq!(limit.period, 100_000, "默认周期 100ms");
        assert!((limit.percent() - 50.0).abs() < 0.01);
    }

    #[test]
    fn percent_above_one_hundred_means_multiple_cores() {
        let limit = CpuLimit::parse("150%").unwrap().unwrap();
        assert_eq!(limit.quota, 150_000, "150% = 1.5 个核");
    }

    #[test]
    fn parses_cores_as_single_number() {
        let limit = CpuLimit::parse("0.25").unwrap().unwrap();
        assert_eq!(limit.quota, 25_000, "0.25 核 = 25000µs");
    }

    #[test]
    fn parses_raw_quota_and_period() {
        let limit = CpuLimit::parse("20000 50000").unwrap().unwrap();
        assert_eq!(limit.to_string(), "20000 50000", "原样落到 cpu.max");
    }

    #[test]
    fn unlimited_words_yield_none() {
        for raw in ["max", "none", "unlimited", "MAX", ""] {
            assert!(CpuLimit::parse(raw).unwrap().is_none(), "{raw} 应视为不限");
        }
    }

    #[test]
    fn rejects_quota_below_kernel_minimum() {
        let error = CpuLimit::parse("0.5%").unwrap_err();
        assert!(
            error.contains("太小"),
            "0.5% 换算成 500µs，低于内核下限: {error}"
        );
    }

    #[test]
    fn rejects_bad_period_range() {
        let error = CpuLimit::parse("50000 100").unwrap_err();
        assert!(error.contains("周期"), "周期小于 1000µs 应报错: {error}");
    }

    #[test]
    fn rejects_non_numeric_input() {
        assert!(CpuLimit::parse("half").is_err());
        assert!(CpuLimit::parse("-50%").is_err(), "负数不能当配额");
        assert!(CpuLimit::parse("0%").is_err(), "0% 会让进程完全跑不动");
    }
}

// ── 内存 ───────────────────────────────────────────────────

mod memory {
    use crate::cgroup::parse_memory;

    #[test]
    fn parses_binary_suffixes() {
        assert_eq!(parse_memory("512M").unwrap(), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory("2G").unwrap(), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_memory("1.5G").unwrap(), Some(1536 * 1024 * 1024));
        assert_eq!(parse_memory("64k").unwrap(), Some(64 * 1024));
    }

    #[test]
    fn parses_bytes_and_spaced_suffix() {
        assert_eq!(parse_memory("1048576").unwrap(), Some(1_048_576));
        assert_eq!(
            parse_memory("512 MB").unwrap(),
            Some(512 * 1024 * 1024),
            "数字与单位之间允许空格"
        );
    }

    #[test]
    fn unlimited_words_yield_none() {
        assert_eq!(parse_memory("max").unwrap(), None);
        assert_eq!(parse_memory("").unwrap(), None);
    }

    #[test]
    fn rejects_unknown_unit_and_tiny_values() {
        let error = parse_memory("512X").expect_err("未知单位必须报错而不是当成字节");
        assert!(
            error.contains('X'),
            "错误里要原样带回用户写错的单位: {error}"
        );
        assert!(parse_memory("1K").is_err(), "1024 字节小于一页，内核会拒绝");
    }
}

// ── 任务数 ─────────────────────────────────────────────────

mod pids {
    use crate::cgroup::parse_pids;

    #[test]
    fn parses_positive_count() {
        assert_eq!(parse_pids("128").unwrap(), Some(128));
        assert_eq!(parse_pids("max").unwrap(), None);
    }

    #[test]
    fn rejects_zero_and_fraction() {
        assert!(parse_pids("0").is_err(), "0 会让进程创建不了任务");
        assert!(parse_pids("1.5").is_err());
    }
}

// ── 组合 ───────────────────────────────────────────────────

mod spec {
    use super::*;

    #[test]
    fn empty_fields_mean_inactive() {
        let spec = CgroupSpec::parse(None, None, Some("  ")).unwrap();
        assert!(!spec.is_active(), "三个字段都为空时不启用 cgroup");
        assert!(spec.controllers().is_empty());
    }

    #[test]
    fn collects_controllers_for_configured_limits() {
        let spec = CgroupSpec::parse(Some("50%"), Some("512M"), Some("64")).unwrap();
        assert!(spec.is_active());
        assert_eq!(spec.controllers(), vec!["cpu", "memory", "pids"]);
    }

    #[test]
    fn describe_is_human_readable() {
        let spec = CgroupSpec::parse(Some("150%"), Some("512M"), Some("64")).unwrap();
        assert_eq!(spec.describe(), "cpu 150%、内存 512.0 MB、pids 64");
    }

    #[test]
    fn reports_field_name_on_parse_error() {
        let error = CgroupSpec::parse(Some("50%"), Some("lot"), None).unwrap_err();
        assert!(
            error.contains("memory_max"),
            "错误里要指出哪个字段: {error}"
        );
    }
}

// ── 命名与格式化 ───────────────────────────────────────────

mod naming {
    use super::*;

    #[test]
    fn slug_replaces_unsafe_characters() {
        let name = slug("web api");
        assert!(name.starts_with("web-api-"), "空格换成连字符: {name}");
        assert_eq!(name.len(), "web-api-".len() + 8, "可读前缀 + 8 位哈希");
        assert!(slug("../../etc")
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert!(
            slug("服务").starts_with("proc-"),
            "全非 ASCII 的进程名也要有可读前缀"
        );
    }

    #[test]
    fn slug_is_stable_and_distinguishes_names() {
        assert_eq!(slug("web"), slug("web"), "同名进程重启要复用同一个目录");
        assert_ne!(slug("web api"), slug("web_api"), "清洗后同名也要能区分");
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(512 * 1024 * 1024), "512.0 MB");
        assert_eq!(format_bytes(999), "999 B");
    }
}

// ── cgroup 文件协议（仅 Linux） ─────────────────────────────
//
// 这里不走内核：用临时目录伪造 `cgroup.controllers` / `cgroup.subtree_control`，
// 验证「写哪个文件、写什么内容」。真正的限额执行由内核负责，只能在有 root 的
// Linux 上验，单元测试覆盖的是协议本身。

#[cfg(target_os = "linux")]
mod linux_protocol {
    use std::fs;
    use std::path::PathBuf;

    use crate::cgroup::linux::{enable_controllers, write_limits};
    use crate::cgroup::{Cgroup, CgroupSpec};
    use crate::tests::support::{tempdir, TempDir};

    fn fake_mount(controllers: &str) -> (TempDir, PathBuf) {
        let dir = tempdir();
        let base = dir.path().join("cgroup2");
        fs::create_dir_all(&base).expect("创建假挂载点");
        fs::write(base.join("cgroup.controllers"), controllers).expect("写 controllers");
        fs::write(base.join("cgroup.subtree_control"), "").expect("写 subtree_control");
        (dir, base)
    }

    fn spec() -> CgroupSpec {
        CgroupSpec::parse(Some("50%"), Some("512M"), Some("32")).expect("限值应能解析")
    }

    #[test]
    fn writes_limits_in_kernel_units() {
        let (_dir, base) = fake_mount("cpu memory pids\n");
        let process = base.join("gugu").join("web");
        fs::create_dir_all(&process).expect("创建进程目录");
        // 内核会提供这两个文件；假目录里先建出来，才能验证我们写的内容
        for file in ["memory.swap.max", "memory.oom.group"] {
            fs::write(process.join(file), "").expect("写占位文件");
        }

        write_limits(&process, &spec()).expect("写限值");

        assert_eq!(
            fs::read_to_string(process.join("cpu.max")).expect("读 cpu.max"),
            "50000 100000",
            "对外写百分比，落到内核的是 quota/period"
        );
        assert_eq!(
            fs::read_to_string(process.join("memory.max")).expect("读 memory.max"),
            "536870912",
            "512M 按 1024 进制换算成字节"
        );
        assert_eq!(
            fs::read_to_string(process.join("memory.swap.max")).expect("读 swap"),
            "0",
            "关掉 swap，避免靠换页绕过上限"
        );
        assert_eq!(
            fs::read_to_string(process.join("memory.oom.group")).expect("读 oom.group"),
            "1",
            "整组一起 OOM"
        );
        assert_eq!(
            fs::read_to_string(process.join("pids.max")).expect("读 pids.max"),
            "32"
        );
    }

    #[test]
    fn enables_only_the_missing_controllers() {
        let (_dir, base) = fake_mount("cpu memory pids\n");
        let group_root = base.join("gugu");
        fs::create_dir_all(&group_root).expect("创建 gugu 目录");
        fs::write(base.join("cgroup.subtree_control"), "+cpu\n").expect("写已启用项");
        fs::write(group_root.join("cgroup.controllers"), "cpu memory pids\n")
            .expect("写 gugu 控制器");
        fs::write(group_root.join("cgroup.subtree_control"), "").expect("写 gugu 下发文件");

        enable_controllers(&base, &group_root, &spec()).expect("启用控制器");

        assert_eq!(
            fs::read_to_string(base.join("cgroup.subtree_control"))
                .expect("读 subtree_control")
                .trim(),
            "+memory +pids",
            "已经启用的 cpu 不重复下发"
        );
        assert_eq!(
            fs::read_to_string(group_root.join("cgroup.subtree_control"))
                .expect("读 gugu subtree_control")
                .trim(),
            "+cpu +memory +pids",
            "`gugu` 自己没有进程，控制器还要再下放一层，进程目录才会出现限值文件"
        );
    }

    #[test]
    fn all_controllers_enabled_writes_nothing() {
        let (_dir, base) = fake_mount("cpu memory pids\n");
        let group_root = base.join("gugu");
        fs::create_dir_all(&group_root).expect("创建 gugu 目录");
        fs::write(base.join("cgroup.subtree_control"), "+cpu +memory +pids\n").expect("写已启用项");
        fs::write(group_root.join("cgroup.controllers"), "cpu memory pids\n")
            .expect("写 gugu 控制器");
        fs::write(
            group_root.join("cgroup.subtree_control"),
            "+cpu +memory +pids\n",
        )
        .expect("写 gugu 下发文件");

        enable_controllers(&base, &group_root, &spec()).expect("启用控制器");

        assert_eq!(
            fs::read_to_string(base.join("cgroup.subtree_control")).expect("读回"),
            "+cpu +memory +pids\n",
            "下发空指令也会被内核拒绝，因此没有缺项就不写"
        );
        assert_eq!(
            fs::read_to_string(group_root.join("cgroup.subtree_control")).expect("读回 gugu"),
            "+cpu +memory +pids\n",
            "`gugu` 层已启用时不重复下发"
        );
    }

    #[test]
    fn reports_controller_missing_from_the_hierarchy() {
        let (_dir, base) = fake_mount("cpu memory\n");
        let group_root = base.join("gugu");
        fs::create_dir_all(&group_root).expect("创建 gugu 目录");

        let error = enable_controllers(&base, &group_root, &spec()).expect_err("缺 pids 应报错");
        assert!(error.contains("pids"), "错误要说清缺哪个控制器: {error}");
    }

    #[test]
    fn rejects_a_path_that_is_not_cgroup_v2() {
        let dir = tempdir();
        let base = dir.path().join("plain-dir");
        fs::create_dir_all(&base).expect("创建普通目录");

        let Err(error) = Cgroup::create(&base, "web", &spec()) else {
            panic!("普通目录不该被当成 cgroup v2");
        };
        assert!(error.contains("cgroup v2"), "错误要指出挂载点不对: {error}");
    }
}
