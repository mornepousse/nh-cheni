/// Run nh and render any top-level error.
///
/// Activation failures recognized by [`nh_nixos::error_clarify::try_clarify`]
/// are printed as a clarified block instead of the default color_eyre report
/// (which includes a misleading `Location:` pointing into nh's own source).
/// Unrecognized errors keep the default color_eyre rendering, unchanged.
fn main() -> std::process::ExitCode {
  if let Err(report) = nh::main() {
    if let Some(block) = nh_nixos::error_clarify::try_clarify(&report) {
      eprintln!("{block}");
      return std::process::ExitCode::FAILURE;
    }
    eprintln!("{report:?}");
    return std::process::ExitCode::FAILURE;
  }
  std::process::ExitCode::SUCCESS
}
