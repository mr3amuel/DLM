// Headless CoreGraphics artwork; no window, Finder or desktop automation.
import Foundation
import CoreGraphics
import CoreText
import ImageIO
import UniformTypeIdentifiers
let width = 660, height = 420
let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
    bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
context.setFillColor(CGColor(gray: 0.97, alpha: 1))
context.fill(CGRect(x: 0, y: 0, width: width, height: height))
let blue = CGColor(srgbRed: 7/255, green: 85/255, blue: 1, alpha: 1)
func text(_ string: String, x: CGFloat, y: CGFloat, size: CGFloat, color: CGColor) {
    let font = CTFontCreateWithName("Helvetica" as CFString, size, nil)
    let attributes: [NSAttributedString.Key: Any] = [
        NSAttributedString.Key(kCTFontAttributeName as String): font,
        NSAttributedString.Key(kCTForegroundColorAttributeName as String): color]
    let line = CTLineCreateWithAttributedString(NSAttributedString(string: string, attributes: attributes))
    context.textPosition = CGPoint(x: x, y: y)
    CTLineDraw(line, context)
}
let logoURL = URL(fileURLWithPath: CommandLine.arguments[1])
let source = CGImageSourceCreateWithURL(logoURL as CFURL, nil)!
let logo = CGImageSourceCreateImageAtIndex(source, 0, nil)!
context.draw(logo, in: CGRect(x: 40, y: 330, width: 56, height: 56))
text("DLM", x: 112, y: 354, size: 28, color: CGColor(gray: 0.12, alpha: 1))
text("Download Manager", x: 112, y: 334, size: 13, color: CGColor(gray: 0.4, alpha: 1))
text("→", x: 310, y: 208, size: 40, color: blue)
text("Drag DLM to Applications", x: 203, y: 64, size: 20, color: blue)
text("Quit DLM before replacing an existing installation.", x: 178, y: 38, size: 12, color: CGColor(gray: 0.4, alpha: 1))
let image = context.makeImage()!
let output = URL(fileURLWithPath: CommandLine.arguments[2])
let destination = CGImageDestinationCreateWithURL(output as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(destination, image, nil)
precondition(CGImageDestinationFinalize(destination))
