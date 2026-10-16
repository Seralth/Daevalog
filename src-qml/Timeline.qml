// SPDX-License-Identifier: AGPL-3.0-only
pragma ComponentBehavior: Bound
import QtQuick
import Daevalog

// The fight on one time axis: a lane per skill with its icon at each hit, then
// the buffs and debuffs on the player (each under the game's own tab), the
// debuffs they put on the target, then the player's stats as steps (yours
// only).
Item {
    id: timeline

    property var player: null
    // The skill lanes: each cast in ms after the fight's start.
    property var lanes: []
    // The buff, debuff and stat tracks (combat/timeline_tracks.rs), times
    // in ms after the fight's start.
    property var tracks: ({ "buffs": [], "stats": [] })
    readonly property var buffs: tracks.buffs.filter(b => b.on === "self" && !b.debuff)
    readonly property var debuffs: tracks.buffs.filter(b => b.on === "self" && b.debuff)
    readonly property var targetDebuffs: tracks.buffs.filter(b => b.on === "target")
    property real seconds: 1
    // The labels' text size (Details' Font size); the names' column and
    // the lanes grow with it.
    property int textSize: Theme.sizeSmall

    readonly property int labelWidth: 190 + (textSize - Theme.sizeSmall) * 10
    readonly property int lane: Math.max(20, textSize + 7)
    // Skill lanes: a skill icon for each hit, at the Details icon size's
    // smallest (a lane per skill, up to 15, stays short).
    readonly property int markSize: 20
    readonly property int skillLane: markSize + 4
    readonly property real trackWidth: width - labelWidth
    readonly property color classColor: Theme.classColor(player ? player.job : "")

    // The fight fits the track while that leaves 20 px a second; a longer
    // fight scrolls sideways under the lane names, since a long fight
    // squeezed in is unreadable.
    readonly property real pxPerSecond: Math.max(trackWidth / Math.max(1, seconds), 20)
    readonly property real contentWidth: Math.max(trackWidth, Math.max(1, seconds) * pxPerSecond)
    readonly property real scrollRange: Math.max(0, contentWidth - trackWidth)
    readonly property bool scrolling: scrollRange > 0.5
    property real scrollX: 0
    onScrollRangeChanged: scrollX = Math.max(0, Math.min(scrollX, scrollRange))
    function scrollBy(delta) {
        scrollX = Math.max(0, Math.min(scrollRange, scrollX + delta))
    }

    // A percent stat's value, sent in hundredths: 600 is 6%, 775 is 7.75%.
    function percentOf(hundredths) {
        return (hundredths / 100).toFixed(2).replace(/\.?0+$/, "") + "%"
    }

    // A lane's hits (ms) as x on the whole fight's track, one per pixel:
    // hits in the same pixel draw the same icon over each other.
    function marks(at) {
        const list = []
        let last = -1
        for (const ms of at) {
            const x = Math.round(Math.min(ms / 1000, Math.max(1, seconds)) * pxPerSecond)
            if (x !== last)
                list.push(x)
            last = x
        }
        return list
    }

    // Where time `t` (seconds) is on a lane's track.
    function xAt(t) {
        return Math.min(t, Math.max(1, seconds)) * pxPerSecond - scrollX
    }

    implicitHeight: content.implicitHeight

    component Section: UiText {
        width: timeline.width
        height: timeline.textSize + 11
        verticalAlignment: Text.AlignBottom
        muted: true
        font.pixelSize: timeline.textSize
        font.weight: Font.DemiBold
    }

    // A lane: its name on the left, the track behind the marks. A lane's
    // marks go in `track` (their Repeater's parent), which clips them
    // while the timeline scrolls.
    component Lane: Item {
        id: laneItem
        property string label
        // Alternate lanes get a faint ground.
        property int stripe: 0
        readonly property alias track: trackItem
        width: timeline.width
        height: timeline.lane
        Rectangle {
            x: timeline.labelWidth
            width: timeline.trackWidth
            height: parent.height
            color: laneItem.stripe % 2 === 0 ? Theme.rowEmpty : "transparent"
        }
        Item {
            id: trackItem
            x: timeline.labelWidth
            width: timeline.trackWidth
            height: parent.height
            clip: timeline.scrolling
        }
        UiText {
            width: timeline.labelWidth - 10
            height: parent.height
            text: laneItem.label
            font.pixelSize: timeline.textSize
        }
    }

    Column {
        id: content
        width: parent.width

        Section {
            text: I18n.tr("details.timelineTracks.skills")
        }
        Repeater {
            model: timeline.lanes
            Lane {
                id: skillLane
                required property var modelData
                required property int index
                stripe: index
                label: modelData.name
                height: Math.max(timeline.skillLane, timeline.lane)
                Repeater {
                    parent: skillLane.track
                    model: timeline.marks(skillLane.modelData.at)
                    SkillIcon {
                        required property real modelData
                        // Centred on the hit, kept inside the fight's span.
                        x: Math.max(0, Math.min(timeline.contentWidth - size, modelData - size / 2)) - timeline.scrollX
                        y: (skillLane.height - size) / 2
                        size: timeline.markSize
                        code: skillLane.modelData.code
                        job: skillLane.modelData.job || (timeline.player ? timeline.player.job : "")
                    }
                }
            }
        }

        Section {
            visible: timeline.buffs.length > 0
            text: I18n.tr("details.timelineTracks.buffs")
        }
        Repeater {
            model: timeline.buffs
            Spans {}
        }
        Section {
            visible: timeline.debuffs.length > 0
            text: I18n.tr("details.timelineTracks.debuffs")
        }
        Repeater {
            model: timeline.debuffs
            Spans { negative: true }
        }
        Section {
            visible: timeline.targetDebuffs.length > 0
            text: I18n.tr("details.timelineTracks.targetDebuffs")
        }
        Repeater {
            model: timeline.targetDebuffs
            Spans {}
        }

        Section {
            visible: timeline.tracks.stats.length > 0
            text: I18n.tr("details.timelineTracks.stats")
        }
        Repeater {
            model: timeline.tracks.stats
            Lane {
                id: statLane
                required property var modelData
                required property int index
                stripe: index
                // Room above the high line for the values.
                height: timeline.lane + timeline.textSize + 7
                label: modelData.name
                readonly property var steps: modelData.steps
                readonly property real low: Math.min(...steps.map(s => s[1]))
                readonly property real high: Math.max(...steps.map(s => s[1]))
                function yOf(v) {
                    return high > low ? timeline.textSize + 4 + (high - v) / (high - low) * (height - timeline.textSize - 8) : height / 2
                }
                Repeater {
                    parent: statLane.track
                    model: statLane.steps.length
                    Item {
                        id: step
                        required property int index
                        readonly property var here: statLane.steps[index]
                        readonly property real end: index + 1 < statLane.steps.length ? statLane.steps[index + 1][0] / 1000 : timeline.seconds
                        Rectangle {
                            x: timeline.xAt(step.here[0] / 1000)
                            y: statLane.yOf(step.here[1])
                            width: timeline.xAt(step.end) - x
                            height: 1.5
                            color: Theme.share
                        }
                        // The rise or fall into the next value.
                        Rectangle {
                            visible: step.index + 1 < statLane.steps.length
                            x: timeline.xAt(step.end)
                            y: Math.min(statLane.yOf(step.here[1]), visible ? statLane.yOf(statLane.steps[step.index + 1][1]) : 0)
                            width: 1
                            height: visible ? Math.abs(statLane.yOf(step.here[1]) - statLane.yOf(statLane.steps[step.index + 1][1])) : 0
                            color: Theme.faint
                        }
                        // The value, where it fits before the next step's.
                        UiText {
                            visible: step.index + 1 === statLane.steps.length || timeline.xAt(step.end) - timeline.xAt(step.here[0] / 1000) >= implicitWidth + 6
                            x: timeline.xAt(step.here[0] / 1000) + 3
                            y: statLane.yOf(step.here[1]) - height - 1
                            height: timeline.textSize + 2
                            numeric: true
                            muted: true
                            font.pixelSize: timeline.textSize
                            text: statLane.modelData.percent ? timeline.percentOf(step.here[1]) : Format.whole(step.here[1])
                        }
                    }
                }
            }
        }

        Item {
            width: timeline.width
            height: 4
        }
        Item {
            x: timeline.labelWidth
            width: timeline.trackWidth
            height: axis.height
            clip: timeline.scrolling
            TimeAxis {
                id: axis
                x: -timeline.scrollX
                width: timeline.contentWidth
                seconds: timeline.seconds
                textSize: timeline.textSize
            }
        }
        UiScrollBar {
            x: timeline.labelWidth
            width: timeline.trackWidth
            position: timeline.scrollX
            range: timeline.scrollRange
            view: timeline.trackWidth
            onMoved: p => timeline.scrollX = p
        }
    }

    // Shift and the wheel, or a sideways swipe, scroll a long fight.
    WheelHandler {
        target: null
        enabled: timeline.scrolling
        acceptedModifiers: Qt.ShiftModifier
        onWheel: event => timeline.scrollBy(-(event.angleDelta.y + event.angleDelta.x) / 2)
    }
    WheelHandler {
        target: null
        enabled: timeline.scrolling
        orientation: Qt.Horizontal
        onWheel: event => timeline.scrollBy(-event.angleDelta.x / 2)
    }

    // A buff or debuff: a bar for each time it was up.
    component Spans: Lane {
        id: spanLane
        // A debuff on the player: Theme.negative, hatched, no end edge.
        property bool negative: false
        readonly property color tint: negative ? Theme.negative : timeline.classColor
        required property var modelData
        required property int index
        stripe: index
        label: modelData.name
        Repeater {
            parent: spanLane.track
            model: spanLane.modelData.spans
            Rectangle {
                required property var modelData
                x: timeline.xAt(modelData[0] / 1000)
                y: 3
                width: timeline.xAt(modelData[1] / 1000) - x
                height: timeline.lane - 6
                radius: 2
                // Only the hatched bars clip: clipping stops Qt batching.
                clip: spanLane.negative
                color: Theme.barFill(spanLane.tint)
                Rectangle {
                    anchors.right: parent.right
                    width: 2
                    height: parent.height
                    visible: !spanLane.negative
                    color: Theme.barEdge(spanLane.tint)
                }
                Canvas {
                    visible: spanLane.negative
                    anchors.fill: parent
                    readonly property color tint: spanLane.tint
                    onTintChanged: requestPaint()
                    onWidthChanged: requestPaint()
                    onHeightChanged: requestPaint()
                    onPaint: {
                        const g = getContext("2d")
                        g.clearRect(0, 0, width, height)
                        g.strokeStyle = Theme.css(tint)
                        g.lineWidth = 2
                        for (let x = -height; x < width; x += 6) {
                            g.beginPath()
                            g.moveTo(x, height)
                            g.lineTo(x + height, 0)
                            g.stroke()
                        }
                    }
                }
            }
        }
    }
}
