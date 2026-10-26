# Draws the Movie Match icon: a white rounded border around a black square holding
# two staggered "m"s with a rainbow gradient. Writes icon.png (256 px, used for the
# window) and icon.ico (16-256 px, embedded in movie-match.exe) next to this script.
#
# Run from PowerShell in any folder:  & .\app\assets\make-icon.ps1

Add-Type -AssemblyName System.Drawing
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# Everything is laid out on a 256 x 256 design grid and scaled per size.
$border = 18          # white border width
$outline = 3          # thin dark line around the white border
$outerRadius = 46
$innerRadius = 30
$letterWidth = 118    # one "m", centre of left stem to centre of right stem
$letterHeight = 62    # from the top of the arches to the bottom of the stems
$stroke = 19          # thickness of the "m" strokes
$stagger = @{ X = 24; Y = 42 }  # each "m" sits this far from the centre, diagonally

function New-RoundedRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $path.AddArc($x, $y, $d, $d, 180, 90)
    $path.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $path.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $path.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $path
}

# One "m" centred on (cx, cy): a left stem, then two round arches down to the baseline.
function Add-M([System.Drawing.Drawing2D.GraphicsPath]$path, [float]$cx, [float]$cy) {
    $r = $letterWidth / 4
    $left = $cx - $letterWidth / 2
    $top = $cy - $letterHeight / 2
    $bottom = $cy + $letterHeight / 2
    $path.StartFigure()
    $path.AddLine($left, $bottom, $left, $top + $r)
    $path.AddArc($left, $top, 2 * $r, 2 * $r, 180, 180)
    $path.AddLine($left + 2 * $r, $top + $r, $left + 2 * $r, $bottom)
    $path.StartFigure()
    $path.AddLine($left + 2 * $r, $bottom, $left + 2 * $r, $top + $r)
    $path.AddArc($left + 2 * $r, $top, 2 * $r, 2 * $r, 180, 180)
    $path.AddLine($left + 4 * $r, $top + $r, $left + 4 * $r, $bottom)
}

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.ScaleTransform($size / 256.0, $size / 256.0)

    # White border with a thin dark outline, then the black square inside it.
    $half = $outline / 2
    $outer = New-RoundedRect $half $half (256 - $outline) (256 - $outline) $outerRadius
    $g.FillPath([System.Drawing.Brushes]::White, $outer)
    $outlinePen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 24, 24, 24)), $outline
    $g.DrawPath($outlinePen, $outer)
    $inset = $outline + $border
    $inner = New-RoundedRect $inset $inset (256 - 2 * $inset) (256 - 2 * $inset) $innerRadius
    $g.FillPath([System.Drawing.Brushes]::Black, $inner)

    # Two staggered "m"s, centred as a pair on the black square.
    $letters = New-Object System.Drawing.Drawing2D.GraphicsPath
    Add-M $letters (128 - $stagger.X) (128 - $stagger.Y)
    Add-M $letters (128 + $stagger.X) (128 + $stagger.Y)

    # One rainbow running diagonally across both letters: red where the top "m" starts,
    # violet where the bottom one ends. Past either end the gradient is mirrored, so the
    # round stroke ends keep the end colour instead of wrapping to the other one.
    $bounds = $letters.GetBounds()
    $start = New-Object System.Drawing.PointF $bounds.Left, $bounds.Top
    $end = New-Object System.Drawing.PointF $bounds.Right, $bounds.Bottom
    $rainbow = New-Object System.Drawing.Drawing2D.LinearGradientBrush $start, $end, ([System.Drawing.Color]::Red), ([System.Drawing.Color]::Violet)
    $rainbow.WrapMode = 'TileFlipXY'
    $blend = New-Object System.Drawing.Drawing2D.ColorBlend 7
    $blend.Colors = @(
        [System.Drawing.Color]::FromArgb(255, 255, 59, 48),   # red
        [System.Drawing.Color]::FromArgb(255, 255, 149, 0),   # orange
        [System.Drawing.Color]::FromArgb(255, 255, 214, 10),  # yellow
        [System.Drawing.Color]::FromArgb(255, 52, 199, 89),   # green
        [System.Drawing.Color]::FromArgb(255, 10, 132, 255),  # blue
        [System.Drawing.Color]::FromArgb(255, 94, 92, 230),   # indigo
        [System.Drawing.Color]::FromArgb(255, 191, 90, 242)   # violet
    )
    $blend.Positions = @(0.0, 0.17, 0.33, 0.5, 0.67, 0.83, 1.0)
    $rainbow.InterpolationColors = $blend

    $pen = New-Object System.Drawing.Pen $rainbow, $stroke
    $pen.StartCap = 'Round'
    $pen.EndCap = 'Round'
    $pen.LineJoin = 'Round'
    $g.DrawPath($pen, $letters)

    $g.Dispose()
    $bmp
}

function Get-PngBytes([System.Drawing.Bitmap]$bmp) {
    $stream = New-Object System.IO.MemoryStream
    $bmp.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    , $stream.ToArray()
}

# A classic 32-bit ICO bitmap entry: header, bottom-up BGRA pixels, then an empty
# AND mask (transparency comes from the alpha channel).
function Get-DibBytes([System.Drawing.Bitmap]$bmp) {
    $size = $bmp.Width
    $maskRow = [int][Math]::Ceiling($size / 32.0) * 4
    $stream = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter $stream
    $bw.Write([UInt32]40); $bw.Write([Int32]$size); $bw.Write([Int32](2 * $size))
    $bw.Write([UInt16]1); $bw.Write([UInt16]32); $bw.Write([UInt32]0)
    $bw.Write([UInt32](4 * $size * $size + $maskRow * $size))
    $bw.Write([Int32]0); $bw.Write([Int32]0); $bw.Write([UInt32]0); $bw.Write([UInt32]0)
    for ($y = $size - 1; $y -ge 0; $y--) {
        for ($x = 0; $x -lt $size; $x++) {
            $c = $bmp.GetPixel($x, $y)
            $bw.Write([byte]$c.B); $bw.Write([byte]$c.G); $bw.Write([byte]$c.R); $bw.Write([byte]$c.A)
        }
    }
    $bw.Write((New-Object byte[] ($maskRow * $size)))
    $bw.Flush()
    , $stream.ToArray()
}

# icon.png for the window.
$big = New-IconBitmap 256
$big.Save((Join-Path $here 'icon.png'), [System.Drawing.Imaging.ImageFormat]::Png)

# icon.ico: bitmap entries for the small sizes (read by every Windows tool) and PNG for 256.
$sizes = 16, 24, 32, 48, 64, 128, 256
$images = foreach ($size in $sizes) {
    $bmp = New-IconBitmap $size
    if ($size -ge 256) { , (Get-PngBytes $bmp) } else { , (Get-DibBytes $bmp) }
}
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([UInt16]0); $w.Write([UInt16]1); $w.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }  # 0 means 256 in ICO headers
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([UInt16]1); $w.Write([UInt16]32)
    $w.Write([UInt32]$images[$i].Length); $w.Write([UInt32]$offset)
    $offset += $images[$i].Length
}
foreach ($bytes in $images) { $w.Write([byte[]]$bytes) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $here 'icon.ico'), $out.ToArray())
"Wrote icon.png and icon.ico in $here"
