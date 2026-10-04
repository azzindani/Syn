// Which charts read from cells that are about to go.
//
// A chart keeps no copy of its numbers: its series are references. A run
// drew a line chart over a helper copy of a table, then deleted the helper
// columns as tidying, and the chart became `Range!#REF!`, one dot in its
// picture, and also what the Dashboard, the report and the deck carried.
// The delete said what the cells held and nothing about who read them.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text.RegularExpressions;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        private static readonly Regex SeriesRef = new(
            @"('(?:[^']|'')+'|[A-Za-z0-9_\.]+)!(\$?[A-Za-z]{1,3}\$?\d+(?::\$?[A-Za-z]{1,3}\$?\d+)?|\$?[A-Za-z]{1,3}:\$?[A-Za-z]{1,3}|\$?\d+:\$?\d+)",
            RegexOptions.Compiled);

        /// <summary>The charts (sheet, chart name) whose series read from
        /// <paramref name="doomed"/> on <paramref name="sheet"/>; with no range,
        /// from anywhere on that sheet. Charts on the sheet itself count when a
        /// range is given (they read cells that are going) and not when the
        /// whole sheet is (they go with it).</summary>
        private static List<(string sheet, string chart)> ChartsReadingFrom(dynamic wb, string sheet, dynamic? doomed)
        {
            var hits = new List<(string sheet, string chart)>();
            try
            {
                int n = (int)wb.Worksheets.Count;
                for (var i = 1; i <= n; i++)
                {
                    dynamic ws = wb.Worksheets[i];
                    var wsName = (string)ws.Name;
                    if (doomed == null && wsName.Equals(sheet, StringComparison.OrdinalIgnoreCase)) continue;
                    int cc;
                    try { cc = (int)ws.ChartObjects().Count; } catch (Exception) { continue; }
                    for (var k = 1; k <= cc; k++)
                    {
                        dynamic co = ws.ChartObjects(k);
                        var reads = false;
                        try
                        {
                            int ns = (int)co.Chart.SeriesCollection().Count;
                            for (var s = 1; s <= ns && !reads; s++)
                            {
                                var f = (string)co.Chart.SeriesCollection(s).Formula;
                                foreach (Match m in SeriesRef.Matches(f))
                                {
                                    var on = m.Groups[1].Value;
                                    if (on.StartsWith('\'')) on = on.Trim('\'').Replace("''", "'");
                                    if (!on.Equals(sheet, StringComparison.OrdinalIgnoreCase)) continue;
                                    if (doomed == null) { reads = true; break; }
                                    try
                                    {
                                        dynamic r = wb.Worksheets[sheet].Range[m.Groups[2].Value.Replace("$", "")];
                                        dynamic x = wb.Application.Intersect(r, doomed);
                                        if (x != null) { reads = true; break; }
                                    }
                                    catch (Exception) { }
                                }
                            }
                        }
                        catch (Exception) { }
                        if (reads) hits.Add((wsName, (string)co.Name));
                    }
                }
            }
            catch (Exception) { }
            return hits;
        }

        /// <summary>The sentence for a reply, or nothing.</summary>
        private static string ChartWarning(List<(string sheet, string chart)> hits) =>
            hits.Count == 0 ? "" :
            $" -- WARNING: {hits.Count.ToString(CultureInfo.InvariantCulture)} chart(s) read from these cells ({string.Join(", ", hits.Take(6).Select(h => h.sheet + "!" + h.chart))}" +
            $"{(hits.Count > 6 ? ", ..." : "")}) and will lose that data (a chart keeps references, not a copy). If that was not meant, undo puts the cells and the charts' links back";

        /// <summary>The state of each of those charts, kept for `undo`: Excel
        /// leaves their series as #REF! when the cells go, and putting the
        /// cells back does not reconnect them.</summary>
        private static List<ChartState> CaptureCharts(dynamic wb, List<(string sheet, string chart)> hits)
        {
            var states = new List<ChartState>();
            foreach (var (sheet, chart) in hits)
                try { states.Add(CaptureChart((object)Sheet(wb, sheet).Shapes.Item(chart), sheet)); } catch (Exception) { }
            return states;
        }
    }
}
