#!/usr/bin/env swift
//
// Draw Otag.app's icon.
//
// The icon is drawn rather than shipped as a file: it is two panes, which is
// the whole idea of otag, and a few lines of Core Graphics say that more
// clearly than a binary asset nobody can edit. It follows Apple's shape —
// a rounded square with a little margin — in otag's own calm colours.
//
// Usage: swift scripts/make-icon.swift <output.icns>

import AppKit
import CoreGraphics
import Foundation

/// The sizes an `.icns` is made of: each at one and two pixels per point.
let sizes = [16, 32, 128, 256, 512]

/// otag's calm palette, as the icon uses it.
let background = (top: CGColor(red: 0.16, green: 0.18, blue: 0.21, alpha: 1),
                  bottom: CGColor(red: 0.10, green: 0.11, blue: 0.13, alpha: 1))
let paneFill = CGColor(red: 1, green: 1, blue: 1, alpha: 0.10)
let paneLine = CGColor(red: 0.56, green: 0.72, blue: 0.75, alpha: 0.85)
let accent = CGColor(red: 0.82, green: 0.63, blue: 0.42, alpha: 1)

/// Draw the icon at one size, as PNG data.
func draw(side: Int) -> Data? {
    let size = CGFloat(side)
    guard let space = CGColorSpace(name: CGColorSpace.sRGB),
          let context = CGContext(
              data: nil,
              width: side,
              height: side,
              bitsPerComponent: 8,
              bytesPerRow: 0,
              space: space,
              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
          )
    else { return nil }

    // The rounded square, inset the way macOS icons are, with a soft
    // vertical gradient so it does not read as flat.
    let margin = size * 0.09
    let body = CGRect(x: margin, y: margin, width: size - margin * 2, height: size - margin * 2)
    let radius = body.width * 0.225
    let shape = CGPath(roundedRect: body, cornerWidth: radius, cornerHeight: radius,
                       transform: nil)

    context.saveGState()
    context.addPath(shape)
    context.clip()
    if let gradient = CGGradient(
        colorsSpace: space,
        colors: [background.top, background.bottom] as CFArray,
        locations: [0, 1]
    ) {
        context.drawLinearGradient(
            gradient,
            start: CGPoint(x: 0, y: body.maxY),
            end: CGPoint(x: 0, y: body.minY),
            options: []
        )
    }

    // Two panes side by side, the right one marked: one agent working, one
    // wanting you.
    let inset = body.width * 0.17
    let area = body.insetBy(dx: inset, dy: inset)
    let gap = area.width * 0.07
    let paneWidth = (area.width - gap) / 2
    let left = CGRect(x: area.minX, y: area.minY, width: paneWidth, height: area.height)
    let right = CGRect(x: area.minX + paneWidth + gap, y: area.minY,
                       width: paneWidth, height: area.height)
    let paneRadius = max(area.width * 0.06, 1)
    let line = max(size * 0.012, 0.75)

    for pane in [left, right] {
        let path = CGPath(roundedRect: pane, cornerWidth: paneRadius,
                          cornerHeight: paneRadius, transform: nil)
        context.addPath(path)
        context.setFillColor(paneFill)
        context.fillPath()
        context.addPath(path)
        context.setStrokeColor(paneLine)
        context.setLineWidth(line)
        context.strokePath()
    }

    // The mark on the right-hand pane: a dot, which is how otag says
    // "this one needs you" everywhere else.
    let dot = min(right.width, right.height) * 0.22
    context.setFillColor(accent)
    context.fillEllipse(in: CGRect(
        x: right.midX - dot / 2,
        y: right.midY - dot / 2,
        width: dot,
        height: dot
    ))
    context.restoreGState()

    guard let image = context.makeImage() else { return nil }
    let representation = NSBitmapImageRep(cgImage: image)
    representation.size = NSSize(width: side, height: side)
    return representation.representation(using: .png, properties: [:])
}

// MARK: - Writing the iconset

let arguments = CommandLine.arguments
guard arguments.count == 2 else {
    FileHandle.standardError.write(Data("usage: make-icon.swift <output.icns>\n".utf8))
    exit(2)
}
let output = URL(fileURLWithPath: arguments[1])
let iconset = output.deletingPathExtension().appendingPathExtension("iconset")

let manager = FileManager.default
try? manager.removeItem(at: iconset)
try manager.createDirectory(at: iconset, withIntermediateDirectories: true)

for size in sizes {
    for scale in [1, 2] {
        guard let data = draw(side: size * scale) else {
            FileHandle.standardError.write(Data("cannot draw the icon at \(size)\n".utf8))
            exit(1)
        }
        let suffix = scale == 1 ? "" : "@2x"
        let name = "icon_\(size)x\(size)\(suffix).png"
        try data.write(to: iconset.appendingPathComponent(name))
    }
}

// `iconutil` is part of macOS; without it the bundle simply has no icon.
let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", output.path]
try iconutil.run()
iconutil.waitUntilExit()
guard iconutil.terminationStatus == 0 else {
    FileHandle.standardError.write(Data("iconutil could not make \(output.path)\n".utf8))
    exit(1)
}
try? manager.removeItem(at: iconset)
print("wrote \(output.path)")
