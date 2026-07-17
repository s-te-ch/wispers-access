import SwiftUI
import UIKit

/// The share's icon as a rounded tile (corner radius = size/3): the cached site
/// favicon if one exists, otherwise a generated green letter tile. Mirrors the
/// Android `ShareAvatar`. (Favicon harvesting isn't wired up yet, so today this
/// is always the letter tile.)
struct ShareAvatar: View {
    let nickname: String
    var iconPNG: Data? = nil
    var size: CGFloat = 48

    var body: some View {
        Group {
            if let iconPNG, let image = UIImage(data: iconPNG) {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFill()
            } else {
                AccessColor.primary.overlay {
                    Text(String(letter))
                        .font(.system(size: size * 0.44, weight: .semibold, design: .serif))
                        .foregroundStyle(AccessColor.primaryDark)
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: size / 3, style: .continuous))
    }

    /// First letter of the name, lowercased; falls back to 'w' (as in the logo).
    private var letter: Character {
        nickname.first { $0.isLetter }?.lowercased().first ?? "w"
    }
}
