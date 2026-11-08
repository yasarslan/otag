/// Build-time availability of features in the macOS app.
public enum AppFeatures {
    /// Enable the team interface once its redesign is ready.
    public static let teamMode = false
}

/// The modes offered by the New session form.
public enum NewSessionMode: Hashable {
    case solo
    case team

    public static var offered: [Self] {
        AppFeatures.teamMode ? [.solo, .team] : [.solo]
    }
}
