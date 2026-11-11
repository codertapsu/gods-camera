package dev.godscamera.android

/** The state of the debugging connection, in debug builds. */
enum class Link { Unknown, Connected, WrongToken, Unreachable }

/** How this device stands with the Mac, as the screen shows it. */
sealed interface Connection {
    /** No Mac has been connected with a code. */
    data object NotPaired : Connection

    /** A Mac is known and is being looked for on the network. */
    data class Searching(val mac: String) : Connection

    data class Connected(val mac: String) : Connection

    /** The Mac did not answer: it is off, asleep, the app is closed, or it is on another network. */
    data class NotFound(val mac: String) : Connection

    /** The Mac answered and turned this device down: it was removed there. */
    data class Refused(val mac: String) : Connection

    /** Debug builds only: the Mac's hub reached through the debugging connection. */
    data class Development(val link: Link) : Connection

    /** The Mac's name, where one is known. */
    val macName: String?
        get() = when (this) {
            is Searching -> mac
            is Connected -> mac
            is NotFound -> mac
            is Refused -> mac
            NotPaired, is Development -> null
        }

    /** Pages can be sent now. */
    val canSend: Boolean
        get() = this is Connected || (this is Development && link == Link.Connected)
}

/** What can happen to the connection. */
sealed interface LinkEvent {
    /** The code was accepted: this device now knows the Mac called [mac]. */
    data class Paired(val mac: String) : LinkEvent

    /** The person had this device forget the Mac. */
    data object Forgotten : LinkEvent

    /** A look for the Mac began. */
    data object CheckStarted : LinkEvent

    /** The Mac answered and accepted this device. */
    data object Reached : LinkEvent

    /** The Mac answered and refused this device. */
    data object TurnedDown : LinkEvent

    /** Nothing answered at any place the Mac could be. */
    data object NoAnswer : LinkEvent
}

/**
 * The connection after [event]. Events that make no sense in a state leave it
 * as it is; a late answer to an old question must not undo a newer state.
 */
fun Connection.after(event: LinkEvent): Connection {
    val mac = macName
    return when (event) {
        is LinkEvent.Paired -> Connection.Searching(event.mac)
        LinkEvent.Forgotten -> if (this is Connection.Development) this else Connection.NotPaired
        // A check behind a working connection is not shown: the screen says
        // "connected" until the Mac fails to answer.
        LinkEvent.CheckStarted -> if (mac == null || this is Connection.Connected) this else Connection.Searching(mac)
        LinkEvent.Reached -> if (mac == null) this else Connection.Connected(mac)
        LinkEvent.TurnedDown -> if (mac == null) this else Connection.Refused(mac)
        LinkEvent.NoAnswer -> if (mac == null || this is Connection.Refused) this else Connection.NotFound(mac)
    }
}
