// Sign in with GitHub against the relay (self-serve). The relay runs the OAuth
// dance; the app opens `/auth/github/start?client=ios` in an
// ASWebAuthenticationSession, receives `arbos://auth/callback?code=…`, and
// exchanges the code for an access + refresh token pair kept in the Keychain.
// Endpoint paths and JSON keys live in `AuthEndpoints` so the backend's final
// shapes are a one-place change.

import AuthenticationServices
import ArbosFFI
import Foundation
import UIKit

enum AuthMode: String {
    case github
    case paircode
}

struct AuthTokens: Codable, Equatable {
    var accessToken: String
    var refreshToken: String
    /// Unix ms; 0 when the relay did not say.
    var expiresAt: Int64
    var relayUrl: String
    var deviceId: String

    /// Less than five minutes left (the relay's refresh rule).
    var isExpiringSoon: Bool {
        expiresAt > 0 && expiresAt - nowMs() < AuthEndpoints.refreshAheadMs
    }
}

struct AuthUser: Codable, Equatable {
    var id: String
    var login: String
    var name: String?
    var avatarUrl: String?
    /// D79 `avatar_color_index_effective` (nil → derived from `id`).
    var avatarColorIndexEffective: Int? = nil
    /// When the relay created this account, and when it last answered a sign-in for it
    /// (`created_at` / `last_login_at` on `/auth/token` and `/me`).
    var createdAt: Int64? = nil
    var lastLoginAt: Int64? = nil

    /// The relay's own answer to "did this account exist before the sign-in that sent me
    /// this?". `upsert_github_user` writes both stamps from one clock on the insert that
    /// creates the account, and moves only `last_login_at` on every sign-in after it, so a
    /// later `last_login_at` means a returning user.
    ///
    /// A payload carrying neither stamp counts as existing. That is the only safe default:
    /// treating an unknown account as new is what shows someone the setup form for a
    /// profile they already chose.
    var accountPredatesThisSignIn: Bool {
        guard let createdAt, let lastLoginAt else { return true }
        return lastLoginAt > createdAt
    }
}

/// `GET /me` credits block.
struct Credits: Codable, Equatable {
    var balanceUsd: Double
    var grantedUsd: Double
    var spentUsd: Double
    var spentTodayUsd: Double
    var dailyCapUsd: Double?
}

/// Relay auth routes (internal/selfserve-auth-api.md, 2026-10-15).
enum AuthEndpoints {
    static let callbackScheme = "arbos"
    static let callback = "arbos://auth/callback"
    /// Relay connector OAuth success/error page redirects here so the auth sheet closes.
    static let connectorCallbackHost = "connectors"
    static func start(deviceName: String) -> String {
        let name = deviceName.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? "iPhone"
        let redirect = callback.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? callback
        return "/auth/github/start?redirect=\(redirect)&device_name=\(name)"
    }
    static let token = "/auth/token"
    static let refresh = "/auth/refresh"
    static let logout = "/auth/logout"
    static let me = "/me"
    /// Refresh when less than this is left on the access token.
    static let refreshAheadMs: Int64 = 5 * 60 * 1000

    static func isConnectorCallback(_ url: URL) -> Bool {
        url.scheme == callbackScheme && url.host == connectorCallbackHost
    }
}

struct AuthError: Error, LocalizedError {
    var message: String
    var errorDescription: String? { message }

    static func humanize(_ type: String, _ message: String) -> String {
        switch type {
        case "invite_required": return "This relay is invite-only. Ask for an invite code."
        case "oauth_denied": return "GitHub sign-in was declined."
        case "oauth_failed": return "GitHub sign-in failed. Try again."
        case "handle_taken": return "That handle is already taken."
        case "bad_handle": return "Handle must be 1–39 letters, numbers, or hyphens."
        case "": return message
        default: return message.isEmpty ? type : message
        }
    }
}

/// Drives the system web sign-in sheet. Ephemeral so no consent alert is shown
/// and no cookies leak between accounts; the relay's tokens are what persist.
@MainActor
final class WebAuth: NSObject, ASWebAuthenticationPresentationContextProviding {
    private var session: ASWebAuthenticationSession?

    func signIn(startURL: URL) async throws -> String {
        let url = try await open(startURL)
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        if let err = items.first(where: { $0.name == "error" })?.value {
            let message = items.first(where: { $0.name == "message" })?.value ?? err
            throw AuthError(message: AuthError.humanize(err, message))
        }
        guard let code = items.first(where: { $0.name == "code" })?.value, !code.isEmpty else {
            throw AuthError(message: "The relay did not return a sign-in code")
        }
        return code
    }

    /// Open `startURL` in the system auth sheet and wait for any `arbos://` callback
    /// (GitHub sign-in or connector OAuth return).
    func open(_ startURL: URL) async throws -> URL {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<URL, Error>) in
            let session = ASWebAuthenticationSession(url: startURL, callbackURLScheme: AuthEndpoints.callbackScheme) { url, error in
                if let error {
                    if (error as? ASWebAuthenticationSessionError)?.code == .canceledLogin {
                        cont.resume(throwing: AuthError(message: "Sign-in cancelled"))
                    } else {
                        cont.resume(throwing: error)
                    }
                    return
                }
                guard let url else {
                    cont.resume(throwing: AuthError(message: "Sign-in produced no callback"))
                    return
                }
                cont.resume(returning: url)
            }
            session.presentationContextProvider = self
            session.prefersEphemeralWebBrowserSession = true
            self.session = session
            if !session.start() {
                cont.resume(throwing: AuthError(message: "Could not open the sign-in sheet"))
            }
        }
    }

    nonisolated func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        MainActor.assumeIsolated {
            let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
            return scenes.flatMap(\.windows).first { $0.isKeyWindow } ?? scenes.first?.windows.first ?? ASPresentationAnchor()
        }
    }
}

// MARK: - Relay calls

extension RelayAPI {
    /// github | paircode. `/healthz` carries `auth_mode` on new relays; older
    /// relays (legacy `auth`) are paircode unless `auth == "github"`.
    func authMode() async -> AuthMode {
        guard let (data, _) = try? await request("GET", "/healthz", auth: false),
              let dto = try? JSONDecoder().decode(HealthDto.self, from: data) else { return .paircode }
        if let raw = dto.auth_mode, let mode = AuthMode(rawValue: raw) { return mode }
        return dto.auth == "github" ? .github : .paircode
    }

    func authStartURL(deviceName: String) -> URL? {
        URL(string: endpoint.httpUrl + AuthEndpoints.start(deviceName: deviceName))
    }

    /// `POST /auth/token {grant_code}` → tokens + user.
    func exchange(grantCode: String) async throws -> (AuthTokens, AuthUser?) {
        let v = try await json("POST", AuthEndpoints.token, body: ["grant_code": grantCode], auth: false)
        return try RelayAPI.parseTokens(v, relayUrl: endpoint.httpUrl)
    }

    /// `POST /auth/refresh {refresh_token}` → rotated pair (the old refresh token dies).
    func refresh(_ tokens: AuthTokens) async throws -> (AuthTokens, AuthUser?) {
        let v = try await json("POST", AuthEndpoints.refresh, body: ["refresh_token": tokens.refreshToken], auth: false)
        return try RelayAPI.parseTokens(v, relayUrl: endpoint.httpUrl)
    }

    /// `GET /me` → user, credits, machine summary, privacy.
    func me() async throws -> (AuthUser, Credits?, MeMachineDto, MeFeaturesDto?, MePrivacyDto?) {
        let v = try await json("GET", AuthEndpoints.me)
        guard JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v) else {
            throw AuthError(message: "Bad /me response")
        }
        let dto: MeResponseDto
        do {
            dto = try JSONDecoder().decode(MeResponseDto.self, from: data)
        } catch {
            NSLog("MeResponseDto: \(error)")
            throw AuthError(message: "Bad /me response")
        }
        let user = RelayAPI.parseUser(dto.user) ?? AuthUser(id: "", login: "github")
        let c = dto.credits
        let credits = Credits(
            balanceUsd: c.balance_usd,
            grantedUsd: c.granted_usd,
            spentUsd: c.spent_usd,
            spentTodayUsd: c.spent_today_usd,
            dailyCapUsd: c.daily_cap_usd
        )
        return (user, credits, dto.machine, dto.features, dto.privacy)
    }

    /// `PATCH /me {name?, login?, group_add?}` → updated user (display name, handle, and/or privacy).
    func patchMe(name: String? = nil, login: String? = nil, groupAdd: String? = nil) async throws -> AuthUser {
        var body: [String: Any] = [:]
        if let name { body["name"] = name }
        if let login { body["login"] = login }
        if let groupAdd { body["group_add"] = groupAdd }
        let v = try await json("PATCH", AuthEndpoints.me, body: body)
        guard JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v),
              let dto = try? JSONDecoder().decode(MeUserResponseDto.self, from: data),
              let user = RelayAPI.parseUser(dto.user) else {
            throw AuthError(message: "Could not update profile")
        }
        return user
    }

    /// `DELETE /me/avatar`: back to the GitHub picture (response carries the updated user).
    func deleteMeAvatar() async throws -> AuthUser {
        let v = try await json("DELETE", "/me/avatar")
        guard JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v),
              let dto = try? JSONDecoder().decode(MeUserResponseDto.self, from: data),
              let user = RelayAPI.parseUser(dto.user) else {
            throw AuthError(message: "Could not remove picture")
        }
        return user
    }

    func logout() async {
        _ = try? await json("POST", AuthEndpoints.logout, body: [String: Any]())
    }

    static func parseTokens(_ v: Any, relayUrl: String) throws -> (AuthTokens, AuthUser?) {
        guard JSONSerialization.isValidJSONObject(v),
              let data = try? JSONSerialization.data(withJSONObject: v),
              let dto = try? JSONDecoder().decode(TokenResponseDto.self, from: data),
              !dto.access_token.isEmpty else {
            throw AuthError(message: "The relay returned no access token")
        }
        return parseTokens(dto, relayUrl: relayUrl)
    }

    static func parseTokens(_ dto: TokenResponseDto, relayUrl: String) -> (AuthTokens, AuthUser?) {
        let expiresAt: Int64 = dto.expires_in > 0 ? nowMs() + dto.expires_in * 1000 : 0
        return (
            AuthTokens(
                accessToken: dto.access_token,
                refreshToken: dto.refresh_token,
                expiresAt: expiresAt,
                relayUrl: relayUrl,
                deviceId: dto.device_id
            ),
            parseUser(dto.user)
        )
    }

    static func parseUser(_ u: [String: Any]?) -> AuthUser? {
        guard let u, JSONSerialization.isValidJSONObject(u),
              let data = try? JSONSerialization.data(withJSONObject: u),
              let dto = try? JSONDecoder().decode(AccountUserDto.self, from: data) else { return nil }
        return parseUser(dto)
    }

    static func parseUser(_ dto: AccountUserDto) -> AuthUser? {
        guard let login = dto.login.flatMap({ $0 }), !login.isEmpty else { return nil }
        return AuthUser(
            id: dto.id,
            login: login,
            name: dto.name.flatMap { $0 },
            avatarUrl: dto.avatar_url.flatMap { $0 },
            createdAt: dto.created_at,
            lastLoginAt: dto.last_login_at.flatMap { $0 }
        )
    }
}

// MARK: - Keychain

extension Keychain {
    private static func authAccount(_ relayUrl: String) -> String { "auth:" + relayUrl }

    static func loadAuth(relayUrl: String) -> AuthTokens? {
        loadData(account: authAccount(relayUrl)).flatMap { try? JSONDecoder().decode(AuthTokens.self, from: $0) }
    }

    static func storeAuth(_ tokens: AuthTokens) throws {
        try storeData(try JSONEncoder().encode(tokens), account: authAccount(tokens.relayUrl))
    }

    static func deleteAuth(relayUrl: String) {
        deleteData(account: authAccount(relayUrl))
    }
}
