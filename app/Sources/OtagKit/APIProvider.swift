import Foundation
import Security

/// Non-secret provider state supplied by the daemon.
public struct APIProviderInfo: Codable, Sendable {
    public var provider = ""
    public var model = ""
    public var models: [String] = []
    public var configured = false
    public var connected: Bool?
    public var balance: String?
    public var keychainService = ""
    public var keychainAccount = ""
    enum CodingKeys: String, CodingKey {
        case provider, model, models, configured, connected, balance
        case keychainService = "keychain_service"
        case keychainAccount = "keychain_account"
    }
    public init() {}
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        provider = try c.decodeIfPresent(String.self, forKey: .provider) ?? ""
        model = try c.decodeIfPresent(String.self, forKey: .model) ?? ""
        models = try c.decodeIfPresent([String].self, forKey: .models) ?? []
        configured = try c.decodeIfPresent(Bool.self, forKey: .configured) ?? false
        connected = try c.decodeIfPresent(Bool.self, forKey: .connected)
        balance = try c.decodeIfPresent(String.self, forKey: .balance)
        keychainService = try c.decodeIfPresent(String.self, forKey: .keychainService) ?? ""
        keychainAccount = try c.decodeIfPresent(String.self, forKey: .keychainAccount) ?? ""
    }
}

/// Native credential storage. No secret is sent to the daemon or settings.
public enum ProviderKeychain {
    public enum Failure: Error { case unavailable, saveFailed }

    /// Save a user-entered credential only in the local Keychain.
    public static func save(_ key: String, service: String, account: String) throws {
        guard !service.isEmpty, !account.isEmpty, !key.isEmpty else { throw Failure.unavailable }
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service, kSecAttrAccount as String: account]
        let data = Data(key.utf8)
        let status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var item = query
            item[kSecValueData as String] = data
            item[kSecAttrSynchronizable as String] = false
            guard SecItemAdd(item as CFDictionary, nil) == errSecSuccess else { throw Failure.saveFailed }
        } else if status != errSecSuccess { throw Failure.saveFailed }
    }
    /// Remove only this provider's explicitly selected Keychain item.
    public static func remove(service: String, account: String) throws {
        guard !service.isEmpty, !account.isEmpty else { throw Failure.unavailable }
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service, kSecAttrAccount as String: account]
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw Failure.saveFailed }
    }
}
