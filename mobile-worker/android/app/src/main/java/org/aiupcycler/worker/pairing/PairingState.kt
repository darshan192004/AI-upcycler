package org.aiupcycler.worker.pairing

/**
 * What the pairing screen should show (spec D-B3/D-B4/D-B5).
 *
 * Kept separate from any `View` so the branching - including the "the two
 * entry paths are not the same thing" rule the spec is emphatic about - is
 * testable on the JVM lane without an Android device.
 */
sealed interface PairingState {

    /** No stored credentials: the QR/PIN entry form is what the user needs. */
    data object Unpaired : PairingState

    /**
     * Credentials are on disk. [fingerprint] is the master CA fingerprint the
     * worker verified during enrollment, shown so the user can confirm at a
     * glance that it is the master they meant.
     */
    data class Paired(val fingerprint: String) : PairingState

    /**
     * An attempt is in flight. Carried as state rather than a flag so the
     * screen cannot show a stale verdict next to a running request.
     */
    data object Working : PairingState

    /**
     * The attempt reached a definite answer from the master. Retrying the same
     * code cannot help - the user needs a fresh session (spec D-B3).
     */
    data class Rejected(val reason: String) : PairingState

    /**
     * The attempt never reached a verdict - no reply, or a malformed one.
     * Retrying the *same* code may well work, which is why this is a separate
     * state from [Rejected] and is rendered differently.
     */
    data class Failed(val reason: String) : PairingState
}

/**
 * What the user typed into the pairing form.
 *
 * The two fields exist because the spec's entry paths differ in what can be
 * verified: a pasted payload carries the full 64-character fingerprint and the
 * CA is checked against all of it, while a hand-typed fingerprint is only ever
 * a prefix and the response's own fingerprint becomes the trusted value
 * (spec D-B3). Collapsing them into one "code" field would silently downgrade
 * the strong path to the weak one.
 */
data class PairingInput(
    /** A pasted `aiupcycler://pair?fp=...&token=...` payload, or a bare fingerprint prefix. */
    val code: String,
    /** The PIN, when it was not carried inside [code]. */
    val pin: String = "",
) {
    /**
     * Whether [code] is a payload at all, or a bare fingerprint prefix.
     *
     * Matched on the scheme, so a truncated paste is still recognised as an
     * *attempt at a payload* rather than being silently downgraded to a prefix
     * check. That matters: a prefix check trusts the response's own
     * fingerprint, so a mangled paste would otherwise be verified against
     * almost nothing while the UI implied the whole code had been checked.
     * [rejectionReason] then reports the payload as malformed instead.
     */
    val codeIsPayload: Boolean
        get() = code.trim().startsWith("aiupcycler://", ignoreCase = true)

    /** The PIN to redeem: the explicit field, or the one inside the payload. */
    fun resolvedPin(): String = if (pin.isNotBlank()) pin.trim() else parsed()?.pin.orEmpty()

    /** The fingerprint to check against: as typed, or as carried by the payload. */
    fun resolvedFingerprint(): String =
        if (codeIsPayload) parsed()?.fingerprint.orEmpty() else code.trim()

    private fun parsed(): PairingUri.Result.Ok? =
        if (!codeIsPayload) null else PairingUri.parse(code.trim()) as? PairingUri.Result.Ok

    /**
     * Why this input cannot be submitted, or `null` if it can.
     *
     * Every case here is answered without touching the network, so an
     * accidental tap cannot spend a single-use PIN. It lives on the input
     * rather than in the screen so the rules are testable off-device, and it is
     * the *only* pre-flight check: [WorkerRuntime.pair] owns the parse-and-post
     * that follows, so there is no second copy to drift.
     *
     * A payload is checked as a unit before either field is used: reporting
     * "enter the PIN" for a paste that lost its tail would send the user
     * looking for a PIN already inside the code they tried to paste.
     */
    fun rejectionReason(): String? = when {
        code.trim().isEmpty() -> "Enter the pairing code shown on the master"
        codeIsPayload && resolvedFingerprint().isEmpty() ->
            "That pairing code is incomplete - scan or paste the whole code"

        resolvedPin().isEmpty() -> "Enter the 6-digit PIN shown on the master"
        resolvedFingerprint().isEmpty() -> "Enter the fingerprint shown on the master"
        else -> null
    }
}
