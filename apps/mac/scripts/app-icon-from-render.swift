// usage: swift scripts/app-icon-from-render.swift <render.png> <out-1024.png> [head width as a fraction of the icon body, default 0.72]
// Makes the macOS app icon from a character render (docs/03 4절 "로고"): flattens the render's background to one
// even color, finds the round head, cuts it out with the brand outline (BrandGeometry, copied below because this
// script stands alone), centers it in the squircle at the asked width, and writes a 1024px PNG.
import AppKit

let args = CommandLine.arguments
let src = NSImage(contentsOfFile: args[1])!
let out = args[2]
let headFraction = args.count > 3 ? (Double(args[3]) ?? 0.72) : 0.72

let cg = src.cgImage(forProposedRect: nil, context: nil, hints: nil)!
let W = cg.width, H = cg.height
var px = [UInt8](repeating: 0, count: W * H * 4)
let read = CGContext(data: &px, width: W, height: H, bitsPerComponent: 8, bytesPerRow: W * 4,
                     space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
read.draw(cg, in: CGRect(x: 0, y: 0, width: W, height: H))

// 1. background per row: the median of the outer 3% of columns on each side (the head never reaches the edges)
let margin = max(4, W * 3 / 100)
func rowBackground(_ y: Int) -> (Double, Double, Double) {
    var r = [Double](), g = [Double](), b = [Double]()
    for x in Array(0..<margin) + Array((W - margin)..<W) {
        let i = (y * W + x) * 4
        r.append(Double(px[i])); g.append(Double(px[i + 1])); b.append(Double(px[i + 2]))
    }
    func med(_ a: [Double]) -> Double { let s = a.sorted(); return s[s.count / 2] }
    return (med(r), med(g), med(b))
}
let bg = (0..<H).map(rowBackground)
var flat = bg[H / 10]  // the even color the whole background becomes: a row near the top, clear of any floor shading

// 2. flatten: scale every pixel by flat / rowBackground so the background is one color everywhere; find the head
var rowHits = [Int](repeating: 0, count: H), colHits = [Int](repeating: 0, count: W)
for y in 0..<H {
    let (br, bgG, bb) = bg[y]
    for x in 0..<W {
        let i = (y * W + x) * 4
        let r = Double(px[i]) * flat.0 / max(br, 1), g = Double(px[i + 1]) * flat.1 / max(bgG, 1), b = Double(px[i + 2]) * flat.2 / max(bb, 1)
        px[i] = UInt8(min(255, r)); px[i + 1] = UInt8(min(255, g)); px[i + 2] = UInt8(min(255, b))
        // grain in the render is a few levels; the head's edge and body are tens of levels off the background
        if abs(r - flat.0) + abs(g - flat.1) + abs(b - flat.2) > 36 { rowHits[y] += 1; colHits[x] += 1 }
    }
}
// The head's box, read so the contact shadow under it cannot stretch it:
//   top    = first row with anything on it (nothing is above the head)
//   bottom = down the middle column, the head's underside is its darkest edge and the shadow right below it is
//            lighter again, so the bottom is the darkest pixel in the lower half of the column
//   sides  = the widest row in the upper 70% of the head (the shadow spreads wider than the head only below that)
func lum(_ x: Int, _ y: Int) -> Double {
    let i = (y * W + x) * 4
    return (Double(px[i]) + Double(px[i + 1]) + Double(px[i + 2])) / 3
}
func differs(_ x: Int, _ y: Int, by: Double = 36) -> Bool {
    let i = (y * W + x) * 4
    return abs(Double(px[i]) - flat.0) + abs(Double(px[i + 1]) - flat.1) + abs(Double(px[i + 2]) - flat.2) > by
}
let minY = rowHits.firstIndex { $0 > W / 100 } ?? 0
let probeRow = min(H - 1, minY + 30)
let probeL = (0..<W).first { differs($0, probeRow) } ?? 0, probeR = (0..<W).reversed().first { differs($0, probeRow) } ?? W - 1
let cx0 = (probeL + probeR) / 2
let roughBottom = (probeRow..<H).first { !differs(cx0, $0, by: 20) } ?? H - 1   // head plus any shadow under it
let lowerHalf = (minY + roughBottom) / 2
let maxY = (lowerHalf...roughBottom).min { lum(cx0, $0) < lum(cx0, $1) }! + 1
var minX = W, maxX = 0
for y in minY...(minY + (maxY - minY) * 7 / 10) {
    if let l = (0..<W).first(where: { differs($0, y) }) { minX = min(minX, l) }
    if let r = (0..<W).reversed().first(where: { differs($0, y) }) { maxX = max(maxX, r) }
}
let head = CGRect(x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1)
// everything outside the head becomes the even color. The brand outline fitted to the head's box leaves out the
// contact shadow and the render's grain, with a 2px feather at the edge.
// BrandGeometry.outline, unit box: x in half-widths from the center, y in heights from the top.
func outline(in r: CGRect) -> CGPath {
    let cx = r.midX, hw = r.width / 2, h = r.height, top = r.minY
    func pt(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: cx + hw * x, y: top + h * y) }
    let yc = 0.58, kx = 0.88, ky = 0.34, by = 0.46, base = 0.44, bx = 0.44
    let p = CGMutablePath()
    p.move(to: pt(0, 0))
    p.addCurve(to: pt(1, yc), control1: pt(kx, 0), control2: pt(1, yc * (1 - ky)))
    p.addCurve(to: pt(base, 1), control1: pt(1, yc + by * (1 - yc)), control2: pt(base + bx, 1))
    p.addLine(to: pt(-base, 1))
    p.addCurve(to: pt(-1, yc), control1: pt(-base - bx, 1), control2: pt(-1, yc + by * (1 - yc)))
    p.addCurve(to: pt(0, 0), control1: pt(-1, yc * (1 - ky)), control2: pt(-kx, 0))
    p.closeSubpath()
    return p
}
// the mask: the outline rendered into an 8-bit buffer, blurred 2px for the feather
let maskCtx = CGContext(data: nil, width: W, height: H, bitsPerComponent: 8, bytesPerRow: W, space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue)!
maskCtx.setFillColor(gray: 0, alpha: 1); maskCtx.fill(CGRect(x: 0, y: 0, width: W, height: H))
maskCtx.setFillColor(gray: 1, alpha: 1)
// CoreGraphics y goes up; the pixel buffer's y goes down. Flip the context so the head's box reads as measured
maskCtx.concatenate(CGAffineTransform(translationX: 0, y: CGFloat(H)).scaledBy(x: 1, y: -1))
maskCtx.addPath(outline(in: head))
maskCtx.fillPath()
let mask = Array(UnsafeBufferPointer(start: maskCtx.data!.assumingMemoryBound(to: UInt8.self), count: W * H))
func coverage(_ x: Int, _ y: Int) -> Double {
    // box blur over ±2px: 0 well outside the edge, 1 well inside
    var sum = 0.0, n = 0.0
    for dy in -2...2 { for dx in -2...2 {
        let xx = min(W - 1, max(0, x + dx)), yy = min(H - 1, max(0, y + dy))
        sum += Double(mask[yy * W + xx]) / 255; n += 1
    } }
    return sum / n
}
for y in 0..<H {
    for x in 0..<W {
        let inside = coverage(x, y)
        if inside >= 1 { continue }
        let t = 1 - inside
        let i = (y * W + x) * 4
        px[i] = UInt8(Double(px[i]) * (1 - t) + flat.0 * t)
        px[i + 1] = UInt8(Double(px[i + 1]) * (1 - t) + flat.1 * t)
        px[i + 2] = UInt8(Double(px[i + 2]) * (1 - t) + flat.2 * t)
    }
}
// the even color becomes the product's paper (#F5F1EA, docs/03) so the squircle reads on a white desktop too;
// the head shifts by the same small ratio and stays cream
let paper = (245.0, 241.0, 234.0)
for i in stride(from: 0, to: W * H * 4, by: 4) {
    px[i] = UInt8(min(255, Double(px[i]) * paper.0 / flat.0))
    px[i + 1] = UInt8(min(255, Double(px[i + 1]) * paper.1 / flat.1))
    px[i + 2] = UInt8(min(255, Double(px[i + 2]) * paper.2 / flat.2))
}
flat = paper
let flattened = CGContext(data: &px, width: W, height: H, bitsPerComponent: 8, bytesPerRow: W * 4,
                          space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!.makeImage()!

// 3. compose: even background, head centered in the 824px body on a 1024 canvas, no drop shadow (the Dock draws one)
let S: CGFloat = 1024, body: CGFloat = 824, inset = (S - body) / 2
let ctx = CGContext(data: nil, width: Int(S), height: Int(S), bitsPerComponent: 8, bytesPerRow: 0,
                    space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
let rect = CGRect(x: inset, y: inset, width: body, height: body)
ctx.addPath(CGPath(roundedRect: rect, cornerWidth: body * 0.2237, cornerHeight: body * 0.2237, transform: nil)); ctx.clip()
ctx.setFillColor(CGColor(red: flat.0 / 255, green: flat.1 / 255, blue: flat.2 / 255, alpha: 1)); ctx.fill(rect)
let scale = body * headFraction / head.width
let drawW = CGFloat(W) * scale, drawH = CGFloat(H) * scale
// CoreGraphics y goes up; the pixel buffer's y goes down
let headCenterX = (head.midX) * scale, headCenterY = (CGFloat(H) - head.midY) * scale
let origin = CGPoint(x: S / 2 - headCenterX, y: S / 2 - headCenterY)
ctx.interpolationQuality = .high
ctx.draw(flattened, in: CGRect(origin: origin, size: CGSize(width: drawW, height: drawH)))
try! NSBitmapImageRep(cgImage: ctx.makeImage()!).representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
print("wrote \(out); head \(Int(head.width))x\(Int(head.height)) at \(Int(head.midX)),\(Int(head.midY)); background \(Int(flat.0)),\(Int(flat.1)),\(Int(flat.2))")
