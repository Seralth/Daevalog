// SPDX-License-Identifier: AGPL-3.0-only
import QtQuick
import QtTest
import ".."

// Settings > Appearance > Meter size: at 100% the meter keeps its own sizes;
// the other sizes grow all of them alike, slim mode too.
Item {
    MeterSizes {
        id: normal
    }
    MeterSizes {
        id: slim
        slim: true
    }

    function sizes(s) {
        return [s.sizeSmall, s.sizeRow, s.rowHeight, s.rowGap, s.headerHeight, s.headsHeight, s.bossBand,
            s.footerHeight, s.iconSize, s.colIcon, s.colGap, s.padLeft, s.padRight]
    }

    TestCase {
        name: "MeterSizes"

        function cleanup() {
            normal.scale = 1
            slim.scale = 1
        }

        function test_100_percent_is_the_meter_as_it_was() {
            compare(sizes(normal), [11, 13, 30, 3, 30, 20, 16, 28, 18, 22, 6, 8, 10])
            compare(sizes(slim), [11, 13, 22, 2, 30, 20, 3, 28, 15, 22, 6, 8, 10])
            // Every length and mark part the meter scales comes back as it is.
            for (const n of [1, 2, 3, 4, 5, 6, 8, 10, 12, 13, 14, 16, 26, 40, 60, 64, 70, 260, 360]) {
                compare(normal.px(n), n, "px " + n)
                compare(normal.part(n), n, "part " + n)
            }
            for (const n of [1.5, 1.75, 2.5, 3.5, 4.5, 9.5, 10.5])
                compare(normal.part(n), n, "part " + n)
        }

        function test_150_percent() {
            normal.scale = 1.5
            slim.scale = 1.5
            compare(sizes(normal), [17, 20, 45, 5, 45, 30, 24, 42, 27, 33, 9, 12, 15])
            compare(sizes(slim), [17, 20, 33, 3, 45, 30, 5, 42, 23, 33, 9, 12, 15])
            compare(normal.px(13), 20, "a target name of 13 px")
            compare(normal.part(3.5), 5.25, "a mark's part is not rounded")
        }

        function test_every_size_grows_with_the_setting() {
            for (const s of [normal, slim]) {
                let last = sizes(s)
                for (const scale of [1.15, 1.3, 1.5]) {
                    s.scale = scale
                    const now = sizes(s)
                    for (let i = 0; i < now.length; i++)
                        verify(now[i] >= last[i], scale + " size " + i)
                    verify(now[2] > last[2], "rows grow at " + scale)
                    verify(now[1] > last[1], "text grows at " + scale)
                    last = now
                }
            }
        }
    }
}
