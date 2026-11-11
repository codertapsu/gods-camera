import CoreGraphics
import Foundation
import ImageIO
import Vision

/// One line of text found in a picture.
struct Line: Codable {
    var text: String
    /// 0 to 1.
    var confidence: Float
    /// Corners in pixels from the picture's top-left: top-left, top-right,
    /// bottom-right, bottom-left, each as x then y.
    var quad: [Double]
    /// Vision judged the line to be a heading.
    var title: Bool
}

struct Page: Codable {
    var path: String
    var width: Int
    var height: Int
    var lines: [Line]
    var error: String?
}

func usage() -> Never {
    FileHandle.standardError.write(
        Data("usage: gc-ocr [--languages vi-VN,en-US] [--fast] <picture>...\n".utf8))
    exit(2)
}

var languages = ["vi-VN", "en-US"]
var fast = false
var paths: [String] = []
var arguments = Array(CommandLine.arguments.dropFirst())
while !arguments.isEmpty {
    let argument = arguments.removeFirst()
    switch argument {
    case "--languages":
        guard !arguments.isEmpty else { usage() }
        languages = arguments.removeFirst().split(separator: ",").map(String.init)
    case "--fast":
        fast = true
    case "--help", "-h":
        usage()
    default:
        paths.append(argument)
    }
}
guard !paths.isEmpty else { usage() }

func read(_ path: String, languages: [String], fast: Bool) async -> Page {
    var page = Page(path: path, width: 0, height: 0, lines: [], error: nil)
    let url = URL(fileURLWithPath: path)
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
        let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
    else {
        page.error = "not a readable picture"
        return page
    }
    // A phone photo is stored as the sensor saw it, with a tag saying how to
    // turn it. Vision reads it upright when told; the sizes follow suit.
    let raw = (properties[kCGImagePropertyOrientation] as? UInt32) ?? 1
    let orientation = CGImagePropertyOrientation(rawValue: raw) ?? .up
    let turned = [.left, .right, .leftMirrored, .rightMirrored].contains(orientation)
    page.width = turned ? image.height : image.width
    page.height = turned ? image.width : image.height

    var request = RecognizeTextRequest()
    request.recognitionLevel = fast ? .fast : .accurate
    request.usesLanguageCorrection = true
    request.automaticallyDetectsLanguage = false
    let supported = Set(request.supportedRecognitionLanguages.map(\.maximalIdentifier))
    let wanted = languages.map { Locale.Language(identifier: $0) }.filter { supported.contains($0.maximalIdentifier) }
    if !wanted.isEmpty { request.recognitionLanguages = wanted }
    do {
        let observations = try await request.perform(on: image, orientation: orientation)
        let (w, h) = (Double(page.width), Double(page.height))
        for observation in observations {
            guard let best = observation.topCandidates(1).first else { continue }
            // Vision counts from the bottom-left in fractions of the picture.
            let corners = [observation.topLeft, observation.topRight, observation.bottomRight, observation.bottomLeft]
            let quad = corners.flatMap { [Double($0.x) * w, (1 - Double($0.y)) * h] }
            page.lines.append(
                Line(text: best.string, confidence: best.confidence, quad: quad, title: observation.isTitle))
        }
    } catch {
        page.error = "\(error)"
    }
    return page
}

var pages: [Page] = []
for path in paths {
    pages.append(await read(path, languages: languages, fast: fast))
}
let encoder = JSONEncoder()
encoder.outputFormatting = [.sortedKeys]
FileHandle.standardOutput.write(try encoder.encode(pages))
FileHandle.standardOutput.write(Data("\n".utf8))
