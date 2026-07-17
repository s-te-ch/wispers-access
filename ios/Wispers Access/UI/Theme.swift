import SwiftUI

/// The Access palette — mirrors the Android app's `ui/theme/Color.kt`.
enum AccessColor {
    static let background = Color(hex: 0xF7F7F2)
    static let surface = Color.white
    static let primary = Color(hex: 0xA1D283)       // sage green
    static let primaryDark = Color(hex: 0x4A6B36)   // forest — text on the green
    static let onSurface = Color(hex: 0x1A1A1A)
    static let onSurfaceVariant = Color(hex: 0x6B6B68)
    static let outline = Color(hex: 0xD9D9D2)
    static let online = Color(hex: 0x34A853)
    static let infoCard = Color(hex: 0xECE7F1)      // lavender detail cards
    static let destructive = Color(hex: 0xB3261E)   // "Remove share"
}

extension Color {
    init(hex: UInt32) {
        self.init(
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255
        )
    }
}

extension View {
    /// Filled green pill (primary action), forest text — matches the Android
    /// `Button`.
    func accessFilledButton() -> some View {
        font(.body.weight(.medium))
            .foregroundStyle(AccessColor.primaryDark)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 15)
            .background(AccessColor.primary, in: Capsule())
    }

    /// Outlined pill (secondary action) — matches the Android `OutlinedButton`.
    func accessOutlinedButton(tint: Color = AccessColor.onSurface) -> some View {
        font(.body.weight(.medium))
            .foregroundStyle(tint)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 15)
            .overlay(Capsule().stroke(AccessColor.outline, lineWidth: 1))
    }
}
