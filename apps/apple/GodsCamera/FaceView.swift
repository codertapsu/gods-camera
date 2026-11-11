import AppKit
import GodsCameraKit
import SwiftUI

/// The face swap's part of the window: the switch, the saved faces and the options.
struct FaceSection: View {
    @ObservedObject var face: FaceModel
    let status: CameraEngine.Status

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Face").font(.headline)
            Toggle("Swap my face", isOn: Binding(get: { face.wanted }, set: { face.setWanted($0) }))
            statusLine

            ForEach(face.faces) { saved in
                HStack(spacing: 8) {
                    thumbnail(saved.thumbnail)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(saved.name).lineLimit(1)
                        Text(consentLabel(saved.consent)).font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if face.selectedID == saved.id {
                        Image(systemName: "checkmark.circle.fill").foregroundStyle(.tint)
                    }
                    Button(role: .destructive) {
                        face.delete(saved.id)
                    } label: {
                        Image(systemName: "trash")
                    }
                    .buttonStyle(.borderless)
                    .help(String(localized: "Delete this face"))
                }
                .contentShape(Rectangle())
                .onTapGesture { face.select(saved.id) }
            }
            HStack {
                Button("Add a face…") { face.chooseAndAddFace() }.disabled(face.busyAdding)
                if face.busyAdding { ProgressView().controlSize(.small) }
            }
            if let message = face.message {
                Text(message).font(.caption).foregroundStyle(.orange)
            }

            if face.availableQualities.count > 1 {
                Picker(String(localized: "Quality"), selection: $face.quality) {
                    Text("Standard").tag(FaceSwapConfig.Model.inswapper128)
                    Text("Sharper").tag(FaceSwapConfig.Model.hyperswap256)
                }
                .onChange(of: face.quality) { face.qualityChanged() }
            }
            Toggle("Match my skin tone", isOn: $face.settings.colourMatch)
                .onChange(of: face.settings.colourMatch) { face.settingsChanged() }
            Toggle("Show an AI badge to others", isOn: $face.showAIBadge)
                .onChange(of: face.showAIBadge) { face.settingsChanged() }
        }
        .sheet(item: $face.pending) { pending in
            NewFaceSheet(pending: pending, face: face)
        }
        .sheet(isPresented: $face.askingPolicy) {
            PolicySheet(face: face)
        }
    }

    @ViewBuilder private var statusLine: some View {
        switch face.state {
        case .off:
            EmptyView()
        case .loading:
            HStack {
                ProgressView().controlSize(.small)
                Text("Getting the models ready. The first time takes a little longer.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        case .on:
            if let error = status.swapError {
                Label(error, systemImage: "xmark.octagon").font(.caption).foregroundStyle(.red)
            } else if status.swapping {
                Text("Swapping · \(status.swapMs, specifier: "%.0f") ms a face, \(status.swapAgeMs, specifier: "%.0f") ms behind")
                    .font(.caption).monospacedDigit().foregroundStyle(.secondary)
            } else {
                Text(status.faceFound ? "Getting the first swapped face…" : "No face in the picture.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        case .missing(let file):
            Label(
                String(localized: "A model file is not installed: \(file)"), systemImage: "exclamationmark.triangle"
            )
            .font(.caption).foregroundStyle(.orange)
        case .failed(let reason):
            Label(reason, systemImage: "xmark.octagon").font(.caption).foregroundStyle(.red)
        }
    }

    private func thumbnail(_ data: Data) -> some View {
        Group {
            if let image = NSImage(data: data) {
                Image(nsImage: image).resizable().scaledToFill()
            } else {
                Color.gray
            }
        }
        .frame(width: 36, height: 36)
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }
}

func consentLabel(_ consent: FaceConsent) -> String {
    switch consent {
    case .myself: return String(localized: "My own face")
    case .permission: return String(localized: "Used with permission")
    case .synthetic: return String(localized: "AI-generated face")
    }
}

/// Shown after a photo is read: the user names the face and states their right to use it.
private struct NewFaceSheet: View {
    let pending: PendingFace
    @ObservedObject var face: FaceModel
    @State private var name = ""
    @State private var consent: FaceConsent?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Add this face").font(.title3.bold())
            HStack(alignment: .top, spacing: 14) {
                if let image = NSImage(data: pending.thumbnail) {
                    Image(nsImage: image).resizable().frame(width: 96, height: 96)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                }
                VStack(alignment: .leading, spacing: 8) {
                    TextField(String(localized: "Name"), text: $name)
                    if pending.facesInPhoto > 1 {
                        Text("The picture shows \(pending.facesInPhoto) faces. The largest one was used.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
            Text("Whose face is this?").font(.headline)
            Picker("", selection: $consent) {
                Text("This is me.").tag(Optional(FaceConsent.myself))
                Text("It is someone else, and I have their permission.").tag(Optional(FaceConsent.permission))
                Text("It is AI-generated and shows no real person.").tag(Optional(FaceConsent.synthetic))
            }
            .pickerStyle(.radioGroup)
            .labelsHidden()
            Text("If none of these is true, do not add the face.").font(.caption).foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { face.pending = nil }
                Button("Save") {
                    if let consent { face.savePending(name: name, consent: consent) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(consent == nil)
            }
        }
        .padding(20)
        .frame(width: 440)
    }
}

/// Shown once, before the swap is turned on for the first time.
private struct PolicySheet: View {
    @ObservedObject var face: FaceModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Before you swap your face").font(.title3.bold())
            Text("People in a call will see a face that is not the one in front of the camera. Please use this fairly.")
            VStack(alignment: .leading, spacing: 6) {
                Label("Use only your own face, a face you have permission to use, or an AI-generated face.", systemImage: "person.crop.circle.badge.checkmark")
                Label("Do not use it to pass as someone else, to deceive, or to harass.", systemImage: "hand.raised")
                Label("An “AI” badge is drawn on your picture. You can turn it off; consider telling people instead.", systemImage: "sparkles")
                Label("Saved faces are encrypted and stay on this Mac.", systemImage: "lock")
            }
            .font(.callout)
            HStack {
                Spacer()
                Button("Not now", role: .cancel) { face.askingPolicy = false }
                Button("I understand") { face.acceptPolicy() }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 460)
    }
}
