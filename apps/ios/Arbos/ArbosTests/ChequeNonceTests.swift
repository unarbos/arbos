import XCTest
@testable import Arbos

/// The number a new cheque takes (`Wallet.nextNonce`): the chain's next nonce, skipping only
/// numbers held by cheques this device sent that can still be cashed.
final class ChequeNonceTests: XCTestCase {
    private let now: UInt64 = 1_800_000_000

    private func cheque(_ nonce: UInt64, expiresIn seconds: Int64) -> Wallet.Issued {
        Wallet.Issued(nonce: nonce, expiresAt: UInt64(Int64(now) + seconds))
    }

    func testFollowsTheChainAndSkipsOutstandingCheques() {
        XCTAssertEqual(Wallet.nextNonce(base: 5, issued: [], now: now), 5)
        XCTAssertEqual(Wallet.nextNonce(base: 5, issued: [cheque(5, expiresIn: 600), cheque(6, expiresIn: 600)], now: now), 7)
        // The chain moved past both (cashed, or the account spent the numbers elsewhere).
        XCTAssertEqual(Wallet.nextNonce(base: 7, issued: [cheque(5, expiresIn: 600), cheque(6, expiresIn: 600)], now: now), 7)
    }

    func testAnExpiredChequeFreesItsNumberAfterTheMargin() {
        let later = cheque(6, expiresIn: 600)
        // Just expired: the estimate may be early, so the number is still held.
        XCTAssertEqual(Wallet.nextNonce(base: 5, issued: [cheque(5, expiresIn: -60), later], now: now), 7)
        // Well expired: the gap is filled first, or the cheque behind it could never cash.
        XCTAssertEqual(Wallet.nextNonce(base: 5, issued: [cheque(5, expiresIn: -Int64(Wallet.expiryMargin) - 1), later], now: now), 5)
    }
}
