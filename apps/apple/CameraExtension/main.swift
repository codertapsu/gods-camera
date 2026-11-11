import CoreMediaIO
import Foundation

// The app asked this build to stand down so macOS can replace it. Starting
// again now would put the old build back in the way, so leave at once.
if Handover.shouldStayDown { exit(0) }

let providerSource = ProviderSource(clientQueue: nil)
CMIOExtensionProvider.startService(provider: providerSource.provider)
CFRunLoopRun()
