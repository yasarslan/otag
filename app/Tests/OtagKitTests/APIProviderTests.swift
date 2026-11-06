import Foundation
import Testing
@testable import OtagKit

@Suite("API provider configuration")
struct APIProviderTests {
    @Test func configurationProtocolHasNoCredentialField() throws {
        let data = try JSONEncoder().encode(Request.apiProvider(model: "sample-model", refresh: true))
        let json = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(Set(json.keys) == ["type", "provider", "model", "refresh"])
        #expect(json["type"] as? String == "api_provider")
    }
    @Test func missingProviderFieldsDisableSelection() throws {
        let info = try JSONDecoder().decode(APIProviderInfo.self, from: Data("{}".utf8))
        #expect(!info.configured)
        #expect(info.models.isEmpty)
        #expect(info.keychainService.isEmpty)
    }
    @Test func missingMetadataCannotWriteToKeychain() {
        #expect(throws: ProviderKeychain.Failure.self) {
            try ProviderKeychain.save("fake-key-for-validation", service: "", account: "")
        }
    }
    @Test func providerIdentitySurvivesProtocolDecoding() throws {
        let pane = try JSONDecoder().decode(PaneInfo.self, from: Data("{\"id\":1,\"agent\":\"deepseek\"}".utf8))
        #expect(pane.agent == .deepseek)
    }
}
