import SwiftUI

/// Serving-node availability indicator: green when reachable, grey when offline,
/// a small spinner while unknown. Mirrors the Android `StatusDot`.
struct StatusDot: View {
    let availability: Availability
    var size: CGFloat = 8

    var body: some View {
        switch availability {
        case .checking:
            ProgressView()
                .controlSize(.mini)
                .frame(width: size, height: size)
        case .online:
            Circle().fill(AccessColor.online).frame(width: size, height: size)
        case .offline:
            Circle().fill(AccessColor.outline).frame(width: size, height: size)
        }
    }
}
