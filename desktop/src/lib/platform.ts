/// macOS draws the window's content under its title bar, so the top of the
/// window is ours to pad and to drag by. Elsewhere the native title bar does
/// both.
export const underTitleBar = navigator.userAgent.includes("Mac");

/// The drag region attribute's value, absent where the title bar drags.
export const dragRegion = underTitleBar ? "deep" : undefined;

/// How the system has the user allow a change to the certificate store, for
/// texts that announce it. Linux has no store this app changes.
export const certificateStoreAsks = navigator.userAgent.includes("Mac")
  ? "macOS will ask for your password."
  : "Windows will ask you to confirm.";
