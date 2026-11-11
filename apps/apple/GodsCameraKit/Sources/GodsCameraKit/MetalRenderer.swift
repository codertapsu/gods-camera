import CoreGraphics
import CoreText
import CoreVideo
import Foundation
import Metal
import simd

public enum RenderError: Error {
    case noDevice
    case textureUnavailable
    case commandFailed
}

/// One input picture for a render pass.
public struct RenderLayer {
    public var buffer: CVPixelBuffer
    /// Flip the picture left to right.
    public var mirrored: Bool

    public init(buffer: CVPixelBuffer, mirrored: Bool = false) {
        self.buffer = buffer
        self.mirrored = mirrored
    }
}

/// A small picture drawn over the output, such as the "looping" badge.
public struct RenderOverlay {
    public var texture: MTLTexture
    /// Where it goes, as fractions of the output: x, y, width, height.
    public var rect: SIMD4<Float>
    public var opacity: Float

    public init(texture: MTLTexture, rect: SIMD4<Float>, opacity: Float = 1) {
        self.texture = texture
        self.rect = rect
        self.opacity = opacity
    }
}

/// A swapped face to draw onto the primary picture.
public struct RenderPatch {
    /// A square RGBA picture; its alpha is the mask.
    public var texture: MTLTexture
    /// Maps pixels of the primary picture onto pixels of the patch:
    /// `x' = a x + b y + tx`, `y' = c x + d y + ty`.
    public var frameToPatch: (a: Float, b: Float, tx: Float, c: Float, d: Float, ty: Float)
    public var opacity: Float

    public init(
        texture: MTLTexture, frameToPatch: (a: Float, b: Float, tx: Float, c: Float, d: Float, ty: Float),
        opacity: Float
    ) {
        self.texture = texture
        self.frameToPatch = frameToPatch
        self.opacity = opacity
    }
}

/// Draws camera frames into output buffers on the GPU.
///
/// Each pass scales and crops one picture to fill the output, optionally draws
/// a swapped face onto it, optionally cross-fades it with a second picture,
/// and optionally draws an overlay.
///
/// Metal's device, queue and pipeline can be used from any thread, and a
/// render call keeps nothing between calls.
public final class MetalRenderer: @unchecked Sendable {
    public let device: MTLDevice
    private let queue: MTLCommandQueue
    private let pipeline: MTLRenderPipelineState
    private var textureCache: CVMetalTextureCache!

    private struct Uniforms {
        var uvA = matrix_identity_float3x3
        var uvB = matrix_identity_float3x3
        var mix: Float = 0
        var hasB: Float = 0
        var overlayAlpha: Float = 0
        var padding: Float = 0
        var overlayRect = SIMD4<Float>(0, 0, 1, 1)
        /// a, b, tx and the patch's side in pixels.
        var patchRow0 = SIMD4<Float>(1, 0, 0, 1)
        /// c, d, ty and the patch's opacity; 0 draws no patch.
        var patchRow1 = SIMD4<Float>(0, 1, 0, 0)
        /// Width and height of the primary picture, in pixels.
        var sourceSize = SIMD4<Float>(1, 1, 0, 0)
    }

    // The shader is compiled from source at start-up, so it works the same
    // from the app and from command-line tests.
    private static let shaderSource = """
        #include <metal_stdlib>
        using namespace metal;

        struct VertexOut {
            float4 position [[position]];
            float2 uv;
        };

        struct Uniforms {
            float3x3 uvA;
            float3x3 uvB;
            float mix;
            float hasB;
            float overlayAlpha;
            float padding;
            float4 overlayRect;
            float4 patchRow0;
            float4 patchRow1;
            float4 sourceSize;
        };

        vertex VertexOut gc_vertex(uint vid [[vertex_id]]) {
            const float2 positions[4] = { float2(-1, -1), float2(1, -1), float2(-1, 1), float2(1, 1) };
            const float2 uvs[4] = { float2(0, 1), float2(1, 1), float2(0, 0), float2(1, 0) };
            VertexOut out;
            out.position = float4(positions[vid], 0, 1);
            out.uv = uvs[vid];
            return out;
        }

        fragment float4 gc_fragment(VertexOut in [[stage_in]],
                                    texture2d<float> texA [[texture(0)]],
                                    texture2d<float> texB [[texture(1)]],
                                    texture2d<float> overlay [[texture(2)]],
                                    texture2d<float> patch [[texture(3)]],
                                    constant Uniforms &u [[buffer(0)]]) {
            constexpr sampler s(filter::linear, address::clamp_to_edge);
            float2 uvA = (u.uvA * float3(in.uv, 1)).xy;
            float3 color = texA.sample(s, uvA).rgb;
            if (u.patchRow1.w > 0.0) {
                // The core counts pixels with their centres on whole numbers.
                float2 p = uvA * u.sourceSize.xy - 0.5;
                float2 c = float2(dot(u.patchRow0.xy, p) + u.patchRow0.z, dot(u.patchRow1.xy, p) + u.patchRow1.z);
                float side = u.patchRow0.w;
                if (c.x > -0.5 && c.y > -0.5 && c.x < side - 0.5 && c.y < side - 0.5) {
                    float4 face = patch.sample(s, (c + 0.5) / side);
                    color = mix(color, face.rgb, face.a * u.patchRow1.w);
                }
            }
            if (u.hasB > 0.5) {
                float3 other = texB.sample(s, (u.uvB * float3(in.uv, 1)).xy).rgb;
                color = mix(color, other, u.mix);
            }
            if (u.overlayAlpha > 0.0) {
                float2 o = (in.uv - u.overlayRect.xy) / u.overlayRect.zw;
                if (o.x >= 0.0 && o.y >= 0.0 && o.x <= 1.0 && o.y <= 1.0) {
                    float4 badge = overlay.sample(s, o);
                    color = mix(color, badge.rgb, badge.a * u.overlayAlpha);
                }
            }
            return float4(color, 1);
        }
        """

    public init() throws {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
            throw RenderError.noDevice
        }
        self.device = device
        self.queue = queue
        let library = try device.makeLibrary(source: Self.shaderSource, options: nil)
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: "gc_vertex")
        descriptor.fragmentFunction = library.makeFunction(name: "gc_fragment")
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        CVMetalTextureCacheCreate(kCFAllocatorDefault, nil, device, nil, &textureCache)
    }

    /// Maps output coordinates to source coordinates so the source fills the
    /// output, cropping whichever dimension overflows.
    static func aspectFill(source: PixelSize, output: PixelSize, mirrored: Bool) -> simd_float3x3 {
        let scale = max(
            Float(output.width) / Float(source.width), Float(output.height) / Float(source.height))
        let fx = Float(output.width) / (Float(source.width) * scale)
        let fy = Float(output.height) / (Float(source.height) * scale)
        let sx = mirrored ? -fx : fx
        return simd_float3x3(columns: (
            SIMD3<Float>(sx, 0, 0),
            SIMD3<Float>(0, fy, 0),
            SIMD3<Float>(0.5 - 0.5 * sx, 0.5 - 0.5 * fy, 1)
        ))
    }

    private func texture(for buffer: CVPixelBuffer) throws -> (MTLTexture, CVMetalTexture) {
        var wrapped: CVMetalTexture?
        let size = buffer.pixelSize
        CVMetalTextureCacheCreateTextureFromImage(
            kCFAllocatorDefault, textureCache, buffer, nil, .bgra8Unorm, size.width, size.height, 0, &wrapped)
        guard let wrapped, let texture = CVMetalTextureGetTexture(wrapped) else {
            throw RenderError.textureUnavailable
        }
        return (texture, wrapped)
    }

    /// A texture for swapped-face patches of side `size`.
    public func makePatchTexture(size: Int) -> MTLTexture? {
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba8Unorm, width: size, height: size, mipmapped: false)
        descriptor.usage = .shaderRead
        return device.makeTexture(descriptor: descriptor)
    }

    /// Draw `primary` into `output`, with `patch` drawn onto it, cross-fading
    /// towards `secondary` by `mix` (0 shows only the primary, 1 only the secondary).
    public func render(
        primary: RenderLayer, patch: RenderPatch? = nil, secondary: RenderLayer? = nil, mix: Float = 0,
        overlay: RenderOverlay? = nil, into output: CVPixelBuffer
    ) throws {
        let outputSize = output.pixelSize
        // The wrappers must stay alive until the GPU has finished.
        let (primaryTexture, primaryRef) = try texture(for: primary.buffer)
        let (targetTexture, targetRef) = try texture(for: output)
        var uniforms = Uniforms()
        uniforms.uvA = Self.aspectFill(
            source: primary.buffer.pixelSize, output: outputSize, mirrored: primary.mirrored)

        var secondaryTexture = primaryTexture
        var secondaryRef: CVMetalTexture?
        if let secondary {
            let (texture, ref) = try texture(for: secondary.buffer)
            secondaryTexture = texture
            secondaryRef = ref
            uniforms.uvB = Self.aspectFill(
                source: secondary.buffer.pixelSize, output: outputSize, mirrored: secondary.mirrored)
            uniforms.hasB = 1
            uniforms.mix = min(max(mix, 0), 1)
        }
        if let overlay {
            uniforms.overlayAlpha = overlay.opacity
            uniforms.overlayRect = overlay.rect
        }
        if let patch, patch.opacity > 0 {
            let m = patch.frameToPatch
            let source = primary.buffer.pixelSize
            uniforms.patchRow0 = SIMD4<Float>(m.a, m.b, m.tx, Float(patch.texture.width))
            uniforms.patchRow1 = SIMD4<Float>(m.c, m.d, m.ty, min(patch.opacity, 1))
            uniforms.sourceSize = SIMD4<Float>(Float(source.width), Float(source.height), 0, 0)
        }

        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = targetTexture
        pass.colorAttachments[0].loadAction = .dontCare
        pass.colorAttachments[0].storeAction = .store
        guard let commands = queue.makeCommandBuffer(),
            let encoder = commands.makeRenderCommandEncoder(descriptor: pass)
        else { throw RenderError.commandFailed }
        encoder.setRenderPipelineState(pipeline)
        encoder.setFragmentTexture(primaryTexture, index: 0)
        // Every texture slot the shader declares gets a texture, used or not.
        encoder.setFragmentTexture(secondaryTexture, index: 1)
        encoder.setFragmentTexture(overlay?.texture ?? primaryTexture, index: 2)
        encoder.setFragmentTexture(patch?.texture ?? primaryTexture, index: 3)
        withUnsafeBytes(of: &uniforms) { bytes in
            encoder.setFragmentBytes(bytes.baseAddress!, length: bytes.count, index: 0)
        }
        encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
        encoder.endEncoding()
        commands.commit()
        commands.waitUntilCompleted()
        _ = (primaryRef, targetRef, secondaryRef)
        if commands.status == .error { throw RenderError.commandFailed }
    }

    /// A rounded label with white text, for on-picture badges.
    public func makeBadge(text: String, color: CGColor) -> MTLTexture? {
        let font = CTFontCreateWithName("HelveticaNeue-Bold" as CFString, 44, nil)
        let attributes: [NSAttributedString.Key: Any] = [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): CGColor(gray: 1, alpha: 1),
        ]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
        let bounds = CTLineGetBoundsWithOptions(line, .useOpticalBounds)
        let width = Int(ceil(bounds.width)) + 48
        let height = 80
        guard
            let context = CGContext(
                data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        else { return nil }
        context.setFillColor(color)
        context.addPath(
            CGPath(
                roundedRect: CGRect(x: 0, y: 0, width: width, height: height), cornerWidth: 20,
                cornerHeight: 20, transform: nil))
        context.fillPath()
        context.textPosition = CGPoint(x: 24 - bounds.minX, y: (CGFloat(height) - bounds.height) / 2 - bounds.minY)
        CTLineDraw(line, context)
        guard let data = context.data else { return nil }

        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: width, height: height, mipmapped: false)
        descriptor.usage = .shaderRead
        guard let texture = device.makeTexture(descriptor: descriptor) else { return nil }
        // The context's memory starts with the top row, which is also the texture's first row.
        texture.replace(
            region: MTLRegionMake2D(0, 0, width, height), mipmapLevel: 0, withBytes: data,
            bytesPerRow: width * 4)
        return texture
    }
}
