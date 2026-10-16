// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// DPS over the fight: the player in their class colour, the party in grey.
// The points are evenly spread over `seconds`.
Item {
    id: chart

    property var player: null
    // From the player alone, so the legend never names one that is gone.
    readonly property bool allPlayers: player === null
    property real seconds: 0
    property var playerPoints: []
    property var partyPoints: []
    // The legend's and the axes' text size (Details' Font size).
    property int textSize: Theme.sizeSmall
    // The room the Y axis's numbers take.
    readonly property int axisWidth: textSize * 4

    readonly property var series: ({ "you": playerPoints, "party": partyPoints })
    readonly property real ceiling: {
        let most = 1
        for (const v of series.party.concat(series.you))
            most = Math.max(most, v)
        const step = most > 20000 ? 5000 : (most > 2000 ? 1000 : 100)
        return Math.ceil(most / step) * step
    }
    readonly property color line: allPlayers ? Theme.share : Theme.classColor(player ? player.job : "")

    Row {
        id: legend
        spacing: 16
        height: chart.textSize + 7
        Repeater {
            model: chart.allPlayers ? [[I18n.tr("details.chart.party"), Theme.share]]
                : [[I18n.playerName(chart.player), chart.line], [I18n.tr("details.chart.party"), Theme.valueLine]]
            Row {
                id: entry
                required property var modelData
                spacing: 6
                height: legend.height
                Rectangle {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 12
                    height: 2
                    color: entry.modelData[1]
                }
                UiText {
                    height: legend.height
                    text: entry.modelData[0]
                    muted: true
                    font.pixelSize: chart.textSize
                }
            }
        }
    }

    Canvas {
        id: canvas
        anchors.top: legend.bottom
        anchors.topMargin: 6
        anchors.bottom: axis.top
        x: chart.axisWidth
        width: parent.width - chart.axisWidth
        onPaint: {
            const ctx = getContext("2d")
            ctx.reset()
            ctx.strokeStyle = Theme.css(Theme.hairline)
            ctx.lineWidth = 1
            for (let i = 0; i <= 4; i++) {
                const y = Math.round(i * (height - 1) / 4) + 0.5
                ctx.beginPath()
                ctx.moveTo(0, y)
                ctx.lineTo(width, y)
                ctx.stroke()
            }
            const draw = (values, colour, widthPx) => {
                if (values.length < 2)
                    return
                ctx.strokeStyle = Theme.css(colour)
                ctx.lineWidth = widthPx
                ctx.beginPath()
                values.forEach((v, i) => {
                    const x = i / (values.length - 1) * width
                    const y = height - v / chart.ceiling * height
                    if (i === 0)
                        ctx.moveTo(x, y)
                    else
                        ctx.lineTo(x, y)
                })
                ctx.stroke()
            }
            draw(chart.series.party, chart.allPlayers ? Theme.share : Theme.valueLine, chart.allPlayers ? 2 : 1.5)
            if (!chart.allPlayers)
                draw(chart.series.you, chart.line, 2)
        }
        Connections {
            target: chart
            function onLineChanged() { canvas.requestPaint() }
            function onAllPlayersChanged() { canvas.requestPaint() }
            function onSeriesChanged() { canvas.requestPaint() }
        }
        // A theme change while Details is open: the grid and party lines.
        Connections {
            target: Theme
            function onHairlineChanged() { canvas.requestPaint() }
            function onValueLineChanged() { canvas.requestPaint() }
            function onShareChanged() { canvas.requestPaint() }
        }
    }
    // Y axis: the top and half of it.
    Repeater {
        model: [0, 2, 4]
        UiText {
            required property int modelData
            x: 0
            width: chart.axisWidth - 6
            y: canvas.y + modelData * (canvas.height - 1) / 4 - height / 2
            height: chart.textSize + 3
            horizontalAlignment: Text.AlignRight
            numeric: true
            muted: true
            font.pixelSize: chart.textSize
            text: Format.rate(chart.ceiling * (4 - modelData) / 4)
        }
    }
    TimeAxis {
        id: axis
        anchors.bottom: parent.bottom
        x: canvas.x
        width: canvas.width
        seconds: chart.seconds
        textSize: chart.textSize
    }
}
