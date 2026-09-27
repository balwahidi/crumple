use std::process::Command;

/// A valid 1x1 RGBA PNG.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[test]
fn optimize_three_pngs_then_refuse_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in");
    let out = tmp.path().join("out");
    let report = tmp.path().join("r.jsonl");
    std::fs::create_dir_all(input.join("sub")).unwrap();
    for name in ["a.png", "b.png", "sub/c.png"] {
        std::fs::write(input.join(name), PNG_1X1).unwrap();
    }
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_crumple"))
            .arg("optimize")
            .arg(&input)
            .arg("--out")
            .arg(&out)
            .arg("--report")
            .arg(&report)
            .output()
            .unwrap()
    };

    let first = run();
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    for name in ["a.png", "b.png", "sub/c.png"] {
        assert_eq!(std::fs::read(out.join(name)).unwrap(), PNG_1X1);
    }
    let text = std::fs::read_to_string(&report).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[3]["summary"], true);
    assert_eq!(lines[3]["images"], 3);
    for l in &lines[..3] {
        assert_eq!(l["status"], "kept-original");
        assert_eq!(l["codec"], "original");
        assert_eq!(l["score"], serde_json::Value::Null);
    }

    let second = run();
    assert_eq!(second.status.code(), Some(1), "{second:?}");
    let text = std::fs::read_to_string(&report).unwrap();
    let errors = text
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|v| v["status"] == "error")
        .count();
    assert_eq!(errors, 3);
}

fn crumple(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_crumple"))
        .args(args)
        .output()
        .unwrap()
}

fn records(report: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(report)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn output_dir_inside_input_is_not_rediscovered() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in");
    let out = input.join("out");
    let report = tmp.path().join("r.jsonl");
    std::fs::create_dir_all(&input).unwrap();
    for name in ["a.png", "b.png"] {
        std::fs::write(input.join(name), PNG_1X1).unwrap();
    }
    for _ in 0..2 {
        let o = crumple(&[
            "optimize".as_ref(),
            input.as_os_str(),
            "--out".as_ref(),
            out.as_os_str(),
            "--report".as_ref(),
            report.as_os_str(),
            "--overwrite".as_ref(),
        ]);
        assert_eq!(o.status.code(), Some(0), "{o:?}");
        let recs = records(&report);
        assert_eq!(recs.last().unwrap()["images"], 2, "{recs:?}");
    }
    assert!(!out.join("out").exists());
}

#[test]
fn same_name_under_two_roots_is_an_error_not_an_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let (d1, d2, out) = (
        tmp.path().join("d1"),
        tmp.path().join("d2"),
        tmp.path().join("out"),
    );
    let report = tmp.path().join("r.jsonl");
    std::fs::create_dir_all(&d1).unwrap();
    std::fs::create_dir_all(&d2).unwrap();
    std::fs::write(d1.join("a.png"), PNG_1X1).unwrap();
    // Different case and content: on NTFS/APFS `out/a.png` and `out/A.png` are one file.
    let mut other = PNG_1X1.to_vec();
    other.extend_from_slice(b"trailing");
    std::fs::write(d2.join("A.png"), &other).unwrap();
    for extra in [&[][..], &["--overwrite".as_ref()][..]] {
        let mut args = vec![
            "optimize".as_ref(),
            d1.as_os_str(),
            d2.as_os_str(),
            "--out".as_ref(),
            out.as_os_str(),
            "--report".as_ref(),
            report.as_os_str(),
        ];
        args.extend_from_slice(extra);
        let _ = std::fs::remove_dir_all(&out);
        let o = crumple(&args);
        assert_eq!(o.status.code(), Some(1), "{o:?}");
        let recs = records(&report);
        let errors: Vec<_> = recs.iter().filter(|r| r["status"] == "error").collect();
        assert_eq!(errors.len(), 1, "{recs:?}");
        assert!(errors[0]["reason"].as_str().unwrap().contains("collides"));
        let written: Vec<_> = std::fs::read_dir(&out).unwrap().collect();
        assert_eq!(written.len(), 1);
    }
}

#[test]
fn never_writes_over_an_input() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in");
    std::fs::create_dir_all(input.join("sub")).unwrap();
    std::fs::write(input.join("a.png"), PNG_1X1).unwrap();
    let report = tmp.path().join("r.jsonl");
    // `in/sub/..` is the input folder itself.
    let out = input.join("sub").join("..");
    let o = crumple(&[
        "optimize".as_ref(),
        input.as_os_str(),
        "--out".as_ref(),
        out.as_os_str(),
        "--report".as_ref(),
        report.as_os_str(),
        "--overwrite".as_ref(),
    ]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    let recs = records(&report);
    assert_eq!(
        recs[0]["reason"], "output path equals input path",
        "{recs:?}"
    );
    assert_eq!(std::fs::read(input.join("a.png")).unwrap(), PNG_1X1);

    // A --report path that is one of the input images is a usage error.
    let o = crumple(&[
        "optimize".as_ref(),
        input.as_os_str(),
        "--out".as_ref(),
        tmp.path().join("o2").as_os_str(),
        "--report".as_ref(),
        input.join("a.png").as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    assert_eq!(std::fs::read(input.join("a.png")).unwrap(), PNG_1X1);
}

#[test]
fn usage_errors_exit_2() {
    for bad in [
        &["optimize"][..],
        &["optimize", "x", "--out", "o", "--jobs", "0"],
        &["frobnicate"],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_crumple"))
            .args(bad)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{bad:?}: {o:?}");
    }
    let o = Command::new(env!("CARGO_BIN_EXE_crumple"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "crumple 0.0.0");
}
