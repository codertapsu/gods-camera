import AppKit
import GodsCameraKit
import SwiftUI

/// The Face tab of the Camera screen: the saved faces, adding one, how the
/// swapped face looks, and privacy. The switch itself is in the row of
/// controls under the picture, and the swap's state in the line under it.
struct FacePanel: View {
    @EnvironmentObject private var model: AppModel
    @ObservedObject var face: FaceModel
    @State private var renaming: SavedFace?
    @State private var newName = ""
    @State private var dropTargeted = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            faces
            look
            privacy
        }
        .alert(String(localized: "Rename the face"), isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField(String(localized: "Name"), text: $newName)
            Button("Save") {
                if let renaming { face.rename(renaming.id, to: newName) }
                renaming = nil
            }
            Button("Cancel", role: .cancel) { renaming = nil }
        }
    }

    // MARK: Faces

    private var faces: some View {
        Card(String(localized: "Faces"), symbol: "person.crop.square.on.square.angled") {
            ForEach(face.faces) { saved in
                VStack(alignment: .leading, spacing: 8) {
                    faceRow(saved)
                    if face.selectedID == saved.id { photoStrip(saved) }
                }
                .padding(8)
                .background(
                    face.selectedID == saved.id ? AnyShapeStyle(.tint.opacity(0.12)) : AnyShapeStyle(.clear),
                    in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .contentShape(Rectangle())
                .onTapGesture { face.select(saved.id) }
            }
            HStack {
                Button {
                    face.choosePhotos()
                } label: {
                    Label("Add a face…", systemImage: "plus")
                }
                .disabled(face.busyAdding)
                if face.busyAdding {
                    ProgressView().controlSize(.small)
                    Text("Reading the photos…").font(.callout).foregroundStyle(.secondary)
                }
            }
            Hint(String(localized: "Choose several photos of the same person at once: different angles, expressions and light. Together they give a better likeness than one photo. You can also drop photos here."))
            if let message = face.message {
                Hint(message, symbol: "exclamationmark.triangle", tone: .orange)
            }
        }
        .overlay {
            if dropTargeted {
                RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(.tint, lineWidth: 2)
            }
        }
        .dropDestination(for: URL.self) { urls, _ in
            let pictures = urls.filter { ["jpg", "jpeg", "png", "heic", "heif", "tif", "tiff", "webp"].contains($0.pathExtension.lowercased()) }
            guard !pictures.isEmpty else { return false }
            face.addPhotos(pictures)
            return true
        } isTargeted: { dropTargeted = $0 }
    }

    private func faceRow(_ saved: SavedFace) -> some View {
        HStack(spacing: 10) {
            DataImage(data: saved.thumbnail, side: 44, round: true)
                .overlay(Circle().strokeBorder(face.selectedID == saved.id ? AnyShapeStyle(.tint) : AnyShapeStyle(.clear), lineWidth: 2))
            VStack(alignment: .leading, spacing: 2) {
                Text(saved.name).font(.body.weight(.medium)).lineLimit(1)
                Text("\(photoCount(saved.photos.count)) · \(consentLabel(saved.consent))")
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            if face.selectedID == saved.id {
                Image(systemName: "checkmark.circle.fill").foregroundStyle(.tint)
            }
            Menu {
                Button("Add photos…") { face.choosePhotos(for: saved) }
                    .disabled(saved.photos.count >= SavedFace.maxPhotos)
                Button("Rename…") {
                    newName = saved.name
                    renaming = saved
                }
                Divider()
                Button("Delete", role: .destructive) { face.delete(saved.id) }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .help(String(localized: "More for this face"))
        }
    }

    /// The photos the chosen face is made from, with a way to add more.
    private func photoStrip(_ saved: SavedFace) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(saved.photos) { photo in
                    DataImage(data: photo.thumbnail, side: 34)
                        .contextMenu {
                            Button("Remove this photo", role: .destructive) { face.removePhoto(photo.id, from: saved.id) }
                                .disabled(saved.photos.count < 2)
                        }
                        .help(String(localized: "Control-click to remove this photo"))
                }
                if saved.photos.count < SavedFace.maxPhotos {
                    Button {
                        face.choosePhotos(for: saved)
                    } label: {
                        Image(systemName: "plus")
                            .frame(width: 34, height: 34)
                            .background(.quaternary, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    }
                    .buttonStyle(.plain)
                    .help(String(localized: "Add more photos of this person"))
                }
            }
        }
    }

    // MARK: Options

    private var look: some View {
        Card(String(localized: "How it looks"), symbol: "wand.and.stars") {
            if face.availableQualities.count > 1 {
                Picker(String(localized: "Quality"), selection: $face.quality) {
                    Text("Standard").tag(FaceSwapConfig.Model.inswapper128)
                    Text("Sharper").tag(FaceSwapConfig.Model.hyperswap256)
                }
                .onChange(of: face.quality) { face.qualityChanged() }
            }
            LabeledContent(String(localized: "Sharpness")) {
                Slider(value: $face.settings.sharpen, in: 0...1)
                    .onChange(of: face.settings.sharpen) { face.settingsChanged() }
            }
            Toggle("Match my skin tone", isOn: $face.settings.colourMatch)
                .onChange(of: face.settings.colourMatch) { face.settingsChanged() }
            if face.canKeepObjectsInFront {
                Toggle("Keep hands and objects in front of the face", isOn: $face.settings.keepObjectsInFront)
                    .onChange(of: face.settings.keepObjectsInFront) { face.settingsChanged() }
                    .help(String(localized: "A hand, a cup or a microphone in front of your face stays in front of the swapped face."))
            }
            Toggle("Show my real mouth", isOn: $face.settings.realMouth)
                .onChange(of: face.settings.realMouth) { face.settingsChanged() }
                .help(String(localized: "Your own lips and teeth show through the swapped face. Speech looks more natural; the mouth is then yours, not the other face's."))
        }
    }

    private var privacy: some View {
        Card(String(localized: "Privacy"), symbol: "eye.slash") {
            Toggle("Blur the picture when the swap is not on my face", isOn: $face.settings.blurWhenUnswapped)
                .onChange(of: face.settings.blurWhenUnswapped) { face.settingsChanged() }
            Hint(String(localized: "The swap leaves your face when you turn far to the side, when something hides most of it, or when it is too dark. With this on, others then see a blurred picture and never your own face."))
            Divider()
            Toggle("Show an AI badge to others", isOn: $face.showAIBadge)
                .onChange(of: face.showAIBadge) { face.settingsChanged() }
        }
    }
}

/// The sheets that adding a face and turning the swap on bring up. They belong
/// to the whole Camera screen, since both can start from its row of controls.
struct FaceSheets: ViewModifier {
    @ObservedObject var face: FaceModel

    func body(content: Content) -> some View {
        content
            .sheet(item: $face.pending) { pending in
                NewFaceSheet(pending: pending, face: face)
            }
            .sheet(isPresented: $face.askingPolicy) {
                PolicySheet(face: face)
            }
    }
}

func photoCount(_ count: Int) -> String {
    count == 1 ? String(localized: "1 photo") : String(localized: "\(count) photos")
}

func consentLabel(_ consent: FaceConsent) -> String {
    switch consent {
    case .myself: return String(localized: "My own face")
    case .permission: return String(localized: "Used with permission")
    case .synthetic: return String(localized: "AI-generated face")
    }
}

/// Shown after photos are read: which of them will be used, a name for the
/// face, and the user's right to use it.
private struct NewFaceSheet: View {
    let pending: PendingFace
    @ObservedObject var face: FaceModel
    @State private var name = ""
    @State private var consent: FaceConsent?

    private var isAddition: Bool { pending.target != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(isAddition ? String(localized: "Add photos to \(pending.target?.name ?? "")") : String(localized: "Add this face"))
                .font(.title2.bold())

            ScrollView(.horizontal, showsIndicators: pending.photos.count > 6) {
                HStack(spacing: 10) {
                    ForEach(pending.photos) { photo in
                        VStack(spacing: 5) {
                            DataImage(data: photo.thumbnail, side: 76)
                                .opacity(photo.used ? 1 : 0.35)
                                .overlay(alignment: .bottomTrailing) {
                                    Image(systemName: photo.used ? "checkmark.circle.fill" : "xmark.circle.fill")
                                        .symbolRenderingMode(.palette)
                                        .foregroundStyle(.white, photo.used ? .green : .orange)
                                        .font(.title3)
                                        .offset(x: 5, y: 5)
                                }
                            Text(photo.used ? String(localized: "Used") : String(localized: "Left out"))
                                .font(.caption).foregroundStyle(photo.used ? .secondary : Color.orange)
                        }
                        .help(photo.fileName)
                    }
                }
                .padding(.bottom, 6).padding(.trailing, 6)
            }

            VStack(alignment: .leading, spacing: 6) {
                Text(summary).font(.callout)
                if pending.leftOutCount > 0 {
                    Hint(
                        isAddition
                            ? String(localized: "Photos that show a different person from the saved face are left out.")
                            : String(localized: "Photos that show a different person from most of the others are left out."),
                        symbol: "person.2.slash", tone: .orange)
                }
                if !pending.skipped.isEmpty {
                    Hint(String(localized: "No face was found in: \(pending.skipped.joined(separator: ", "))"), symbol: "questionmark.circle")
                }
                if pending.photos.contains(where: { $0.used && $0.facesInPhoto > 1 }) {
                    Hint(String(localized: "Where a photo shows several faces, the largest one was used."))
                }
            }

            if !isAddition {
                TextField(String(localized: "Name"), text: $name).textFieldStyle(.roundedBorder)
                VStack(alignment: .leading, spacing: 6) {
                    Text("Whose face is this?").font(.headline)
                    Picker("", selection: $consent) {
                        Text("This is me.").tag(Optional(FaceConsent.myself))
                        Text("It is someone else, and I have their permission.").tag(Optional(FaceConsent.permission))
                        Text("It is AI-generated and shows no real person.").tag(Optional(FaceConsent.synthetic))
                    }
                    .pickerStyle(.radioGroup)
                    .labelsHidden()
                    Text("If none of these is true, do not add the face.").font(.callout).foregroundStyle(.secondary)
                }
            }

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { face.pending = nil }
                Button(isAddition ? String(localized: "Add") : String(localized: "Save")) {
                    face.savePending(name: name, consent: consent ?? pending.target?.consent ?? .myself)
                }
                .keyboardShortcut(.defaultAction)
                .disabled(pending.usedCount == 0 || (!isAddition && consent == nil))
            }
        }
        .padding(22)
        .frame(width: 520)
    }

    private var summary: String {
        let total = pending.photos.count
        if pending.usedCount == 0 {
            return String(localized: "None of these photos shows the same person as the saved face.")
        }
        if total == 1 { return String(localized: "One photo will be used.") }
        return String(localized: "\(pending.usedCount) of \(total) photos will be used for this face.")
    }
}

/// Shown once, before the swap is turned on for the first time.
private struct PolicySheet: View {
    @ObservedObject var face: FaceModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Before you swap your face").font(.title2.bold())
            Text("People in a call will see a face that is not the one in front of the camera. Please use this fairly.")
            VStack(alignment: .leading, spacing: 8) {
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
        .padding(22)
        .frame(width: 480)
    }
}
