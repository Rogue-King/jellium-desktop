use crate::{bundle_macos, cef, fs as xfs, mpv, paths, template, version};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const MACOS_APP_NAME: &str = "Jellium Desktop.app";
const FRAMEWORK_NAME: &str = "Chromium Embedded Framework";

pub fn stage_cef(out: &Path, cef: &cef::Cef) -> Result<()> {
    if cef.link_external {
        return Ok(());
    }
    let fw_src = cef.dir.join(format!("{FRAMEWORK_NAME}.framework"));
    let fw_dst = out
        .join("Frameworks")
        .join(format!("{FRAMEWORK_NAME}.framework"));
    std::fs::create_dir_all(out.join("Frameworks"))?;
    if fw_dst.exists() {
        std::fs::remove_dir_all(&fw_dst)?;
    }
    xfs::copy_dir_recursive(&fw_src, &fw_dst)?;
    let new_id = format!("@executable_path/Frameworks/{FRAMEWORK_NAME}.framework/{FRAMEWORK_NAME}");
    run_install_name_tool(&[
        "-id".as_ref(),
        new_id.as_ref(),
        fw_dst.join(FRAMEWORK_NAME).as_os_str(),
    ])?;
    let old = format!("@executable_path/../Frameworks/{FRAMEWORK_NAME}.framework/{FRAMEWORK_NAME}");
    let bin = out.join("jellium-desktop");
    run_install_name_tool(&[
        "-change".as_ref(),
        old.as_ref(),
        new_id.as_ref(),
        bin.as_os_str(),
    ])?;
    // libEGL / libGLESv2 symlinks (CEF expects them next to the binary on macOS).
    link_angle_libraries(
        out,
        Path::new(&format!("Frameworks/{FRAMEWORK_NAME}.framework")),
    )?;
    Ok(())
}

pub fn stage_mpv(out: &Path, mpv_info: &mpv::Mpv, used_external: bool, bin: &Path) -> Result<()> {
    if !used_external {
        let runtime = mpv::runtime_library_name();
        let dst = out.join(runtime);
        xfs::copy_file(&mpv_info.build_dir.join(runtime), &dst)?;
        run_install_name_tool(&[
            "-id".as_ref(),
            format!("@executable_path/{runtime}").as_ref(),
            dst.as_os_str(),
        ])?;
        run_install_name_tool(&[
            "-change".as_ref(),
            format!("@rpath/{runtime}").as_ref(),
            format!("@executable_path/{runtime}").as_ref(),
            bin.as_os_str(),
        ])?;
    }
    Ok(())
}

pub fn install(build_dir: &Path, prefix: &Path, _args: &crate::BuildArgs) -> Result<PathBuf> {
    let app = prefix.join(MACOS_APP_NAME);
    let macos_dir = app.join("Contents").join("MacOS");
    let fw_dir = app.join("Contents").join("Frameworks");
    let resources_dir = app.join("Contents").join("Resources");
    std::fs::create_dir_all(&macos_dir)?;
    if fw_dir.exists() {
        std::fs::remove_dir_all(&fw_dir)?;
    }
    std::fs::create_dir_all(&fw_dir)?;
    std::fs::create_dir_all(&resources_dir)?;

    // Binary + staged libmpv
    let bin_src = build_dir.join("jellium-desktop");
    let bin_dst = macos_dir.join("jellium-desktop");
    xfs::copy_executable(&bin_src, &bin_dst)?;
    xfs::copy_file(
        &build_dir.join("libmpv.2.dylib"),
        &macos_dir.join("libmpv.2.dylib"),
    )?;

    // CEF framework — copy from build/Frameworks/ (already install_name-fixed for
    // build-tree layout; CompleteBundleMac re-rewrites for bundle layout).
    let fw_src = build_dir
        .join("Frameworks")
        .join(format!("{FRAMEWORK_NAME}.framework"));
    let fw_dst = fw_dir.join(format!("{FRAMEWORK_NAME}.framework"));
    xfs::copy_dir_recursive(&fw_src, &fw_dst)?;

    // libEGL / libGLESv2 symlinks (bundle layout: MacOS/ → ../Frameworks/...).
    link_angle_libraries(
        &macos_dir,
        Path::new(&format!("../Frameworks/{FRAMEWORK_NAME}.framework")),
    )?;

    // Info.plist
    let ver = version::read()?;
    let mut vars = HashMap::new();
    vars.insert("APP_VERSION_FULL", ver.full.clone());
    template::configure_file(
        &paths::repo_root()
            .join("resources")
            .join("macos")
            .join("Info.plist.in"),
        &app.join("Contents").join("Info.plist"),
        &vars,
    )?;

    // AppIcon
    let icon_src = paths::repo_root()
        .join("resources")
        .join("macos")
        .join("AppIcon.icns");
    if icon_src.exists() {
        xfs::copy_file(&icon_src, &resources_dir.join("AppIcon.icns"))?;
    }

    // MoltenVK ICD descriptor.
    let icd_dir = resources_dir.join("vulkan").join("icd.d");
    std::fs::create_dir_all(&icd_dir)?;
    let icd_dst = icd_dir.join("MoltenVK_icd.json");
    xfs::copy_file(
        &paths::repo_root()
            .join("resources")
            .join("macos")
            .join("MoltenVK_icd.json"),
        &icd_dst,
    )?;

    // Complete the bundle: dep-walk, install_name rewrites, codesign.
    bundle_macos::complete(&app)?;
    Ok(app)
}

fn link_angle_libraries(directory: &Path, framework: &Path) -> Result<()> {
    for lib in ["libEGL.dylib", "libGLESv2.dylib"] {
        let dst = directory.join(lib);
        if dst.exists() || dst.is_symlink() {
            std::fs::remove_file(&dst)?;
        }
        let target = framework.join("Libraries").join(lib);
        if directory.join(&target).is_file() {
            std::os::unix::fs::symlink(&target, &dst)
                .with_context(|| format!("symlink {} -> {}", dst.display(), target.display()))?;
        }
    }
    Ok(())
}

fn run_install_name_tool(args: &[&std::ffi::OsStr]) -> Result<()> {
    let status = Command::new("install_name_tool")
        .args(args)
        .status()
        .context("spawn install_name_tool")?;
    if !status.success() {
        bail!("install_name_tool failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_links_require_shipped_libraries_and_remove_stale_links() -> Result<()> {
        let root = tempfile::tempdir()?;
        let framework = Path::new("Frameworks/CEF.framework");
        let libraries = root.path().join(framework).join("Libraries");
        std::fs::create_dir_all(&libraries)?;
        std::fs::write(libraries.join("libEGL.dylib"), [])?;
        std::os::unix::fs::symlink("missing", root.path().join("libGLESv2.dylib"))?;
        link_angle_libraries(root.path(), framework)?;
        assert!(root.path().join("libEGL.dylib").is_file());
        assert!(!root.path().join("libGLESv2.dylib").is_symlink());
        std::fs::remove_file(libraries.join("libEGL.dylib"))?;
        link_angle_libraries(root.path(), framework)?;
        assert!(!root.path().join("libEGL.dylib").is_symlink());
        Ok(())
    }
}
