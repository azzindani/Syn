// A chart drawn over a chart that is already there redraws that chart.
//
// `chart` only ever added one. A person who says "add the lowest five to the
// same chart" and then "take them off again" expects the same chart to change;
// the model had no way to change one, so it drew another over the first each
// time, and a workbook came out with three charts stacked on one range and two
// on another. The new chart lands on the old one's cells, so the old one is
// the thing to update: same shape on the sheet, new data, type, title and
// size. What it was is kept for `undo`.
//
// "Over" means the new chart's rectangle covers at least 80% of the smaller of
// the two. A chart given its own range, beside or below the others, is a new
// chart as before.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Runtime.InteropServices;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        private sealed class ChartState
        {
            public string Sheet = "", Shape = "", Title = "";
            public int Type;
            public bool HasTitle, HasLegend;
            public List<string> Series = new();
            public double Left, Top, Width, Height;
        }

        /// <summary>Where a chart anchored at <paramref name="at"/> goes: its
        /// sheet and its rectangle in points. A multi-cell anchor is the whole
        /// rectangle; a single cell keeps Excel's default size.</summary>
        private static (dynamic ws, double left, double top, double w, double h) ChartBox(dynamic wb, string at)
        {
            var (dstSheet, dstAddr) = SplitRange(at);
            if (string.IsNullOrEmpty(dstAddr)) throw new InvalidOperationException("chart needs a destination like Dashboard!A1");
            dynamic dws = Sheet(wb, dstSheet);
            dynamic box = dws.Range[dstAddr];
            double w = 440.0, h = 260.0;
            if (dstAddr.Contains(':'))
            {
                w = (double)box.Width;
                h = (double)box.Height;
            }
            return (dws, (double)box.Left, (double)box.Top, w, h);
        }

        /// <summary>The chart already sitting on the rectangle, if any: the
        /// topmost one that the rectangle covers by 80% or more.</summary>
        private static dynamic? FindChartToReplace(dynamic dws, double left, double top, double w, double h)
        {
            dynamic? best = null;
            var bestZ = -1;
            int n = (int)dws.Shapes.Count;
            for (var i = 1; i <= n; i++)
            {
                dynamic s = dws.Shapes.Item(i);
                try { if ((int)s.HasChart == 0) continue; } catch (Exception) { continue; }
                double l = (double)s.Left, t = (double)s.Top, sw = (double)s.Width, sh = (double)s.Height;
                var ix = Math.Max(0, Math.Min(left + w, l + sw) - Math.Max(left, l));
                var iy = Math.Max(0, Math.Min(top + h, t + sh) - Math.Max(top, t));
                var smaller = Math.Min(w * h, sw * sh);
                if (smaller <= 0 || ix * iy / smaller < 0.8) continue;
                var z = (int)s.ZOrderPosition;
                if (z > bestZ) { best = s; bestZ = z; }
            }
            return best;
        }

        private static ChartState CaptureChart(dynamic shape, string sheet)
        {
            var st = new ChartState { Sheet = sheet, Shape = (string)shape.Name };
            dynamic c = shape.Chart;
            st.Type = (int)c.ChartType;
            st.HasTitle = (bool)c.HasTitle;
            if (st.HasTitle) { try { st.Title = (string)c.ChartTitle.Text; } catch (COMException) { } }
            try { st.HasLegend = (bool)c.HasLegend; } catch (COMException) { }
            int n = (int)c.SeriesCollection().Count;
            for (var i = 1; i <= n; i++) st.Series.Add((string)c.SeriesCollection(i).Formula);
            st.Left = (double)shape.Left; st.Top = (double)shape.Top;
            st.Width = (double)shape.Width; st.Height = (double)shape.Height;
            return st;
        }

        /// <summary>Puts a chart back as it was: its series, type, title,
        /// legend and place. (Axis titles, gridlines and data labels are not
        /// kept: the reply says what undo restores.)</summary>
        private static void RestoreChart(dynamic book, ChartState st)
        {
            dynamic shape = Sheet(book, st.Sheet).Shapes.Item(st.Shape);
            dynamic c = shape.Chart;
            int n = (int)c.SeriesCollection().Count;
            for (var i = n; i >= 1; i--) c.SeriesCollection(i).Delete();
            foreach (var f in st.Series)
            {
                dynamic s = c.SeriesCollection().NewSeries();
                // Name, Values and XValues, not `.Formula`: a series given its
                // references through Formula left Excel refusing Worksheet.Copy of
                // the sheet it reads ("Unable to get the Copy property"), which is
                // what undoing the deletion of that sheet needs.
                var parts = SplitSeriesFormula(f);
                if (parts == null) { s.Formula = f; continue; }
                var (name, cats, values) = parts.Value;
                if (values.Length > 0) s.Values = "=" + values;
                if (cats.Length > 0) s.XValues = "=" + cats;
                if (name.Length > 0) s.Name = name.StartsWith('"') ? name.Trim('"') : "=" + name;
            }
            c.ChartType = st.Type;
            c.HasTitle = st.HasTitle;
            if (st.HasTitle) c.ChartTitle.Text = st.Title;
            try { c.HasLegend = st.HasLegend; } catch (COMException) { }
            shape.Left = st.Left; shape.Top = st.Top; shape.Width = st.Width; shape.Height = st.Height;
        }

        /// <summary>Feeds a chart its source. A first column of whole numbers
        /// that only go up (years, months, ranks) is the axis labels, not a
        /// series: Excel's own rule plots a numeric first column as data, so a
        /// chart "of vehicles by model year" came out with the years as a row
        /// of identical bars beside the counts and an axis reading 1 to 15, and
        /// a model that could not fix it built a helper copy of the table,
        /// charted that, then deleted it. A first column of text, a scatter
        /// chart, or anything that is not strictly rising whole numbers is left
        /// to Excel as before. Returns a sentence for the reply, or nothing.</summary>
        private static string SetChartData(dynamic chart, dynamic sws, string srcAddr, int type)
        {
            dynamic rng = sws.Range[srcAddr];
            int rows = (int)rng.Rows.Count, cols = (int)rng.Columns.Count;
            const int xlXYScatter = -4169;
            if (type == xlXYScatter || cols < 2 || rows < 3 || !FirstColumnIsLabels(rng, rows))
            {
                chart.SetSourceData(rng);
                return "";
            }
            string head = "";
            try { head = (string)rng.Cells[1, 1].Text; } catch (COMException) { }
            dynamic values = rng.Offset(0, 1).Resize(rows, cols - 1);
            dynamic cats = rng.Offset(1, 0).Resize(rows - 1, 1);
            chart.SetSourceData(values, 2); // xlColumns: one series per column
            int n = (int)chart.SeriesCollection().Count;
            for (var i = 1; i <= n; i++) chart.SeriesCollection(i).XValues = cats;
            return $"; its first column ({(head.Length > 0 ? head : "A")}) is the axis labels, not plotted";
        }

        private static bool FirstColumnIsLabels(dynamic rng, int rows)
        {
            try
            {
                object header = rng.Cells[1, 1].Value2;
                if (header is double) return false;
                object cells = rng.Columns[1].Value2;
                if (cells is not object[,] a) return false;
                double last = double.NegativeInfinity;
                for (var r = 2; r <= rows; r++)
                {
                    if (a[r, 1] is not double d || d != Math.Floor(d) || d <= last) return false;
                    last = d;
                }
                return true;
            }
            catch (Exception) { return false; }
        }

        /// <summary>=SERIES(name, categories, values, order) split into its
        /// first three arguments, or null when it does not look like that.
        /// Commas inside quotes or parentheses do not split.</summary>
        private static (string name, string cats, string values)? SplitSeriesFormula(string f)
        {
            var t = f.Trim();
            if (!t.StartsWith("=SERIES(", StringComparison.OrdinalIgnoreCase) || !t.EndsWith(')')) return null;
            var inner = t["=SERIES(".Length..^1];
            var args = new List<string>();
            var sb = new System.Text.StringBuilder();
            int depth = 0; bool quoted = false;
            foreach (var ch in inner)
            {
                if (ch == '"') quoted = !quoted;
                if (!quoted && ch == '(') depth++;
                if (!quoted && ch == ')') depth--;
                if (!quoted && depth == 0 && ch == ',') { args.Add(sb.ToString().Trim()); sb.Clear(); continue; }
                sb.Append(ch);
            }
            args.Add(sb.ToString().Trim());
            if (args.Count < 3) return null;
            return (args[0], args[1], args[2]);
        }

        /// <summary>What kind of chart an Excel ChartType is, in the words
        /// `chart` takes.</summary>
        private static string ChartKindName(int type) => type switch
        {
            4 => "line", 51 => "column", 57 => "bar", 5 => "pie", -4169 => "scatter", 1 => "area",
            -4120 => "doughnut", 52 => "stackedColumn", 58 => "stackedBar", 65 => "lineMarkers", -4151 => "radar",
            _ => "chart",
        };

        /// <summary>Redraws an existing chart as the new one the call asked
        /// for. If anything fails half way, the chart goes back as it was.</summary>
        private static string RedrawChart(dynamic old, dynamic sws, string kind, int type, string srcSheet, string srcAddr,
            string title, string style, string dstSheet, string dstAddr, double left, double top, double w, double h)
        {
            var was = CaptureChart(old, dstSheet);
            dynamic chart = old.Chart;
            var note = "";
            try
            {
                chart.ChartType = type;
                note = SetChartData(chart, sws, srcAddr, type);
                if (!string.IsNullOrWhiteSpace(title))
                {
                    chart.HasTitle = true;
                    chart.ChartTitle.Text = title;
                }
                StyleChart(chart, style);
                old.Left = left; old.Top = top; old.Width = w; old.Height = h;
            }
            catch
            {
                try { RestoreChart(wbOf(old), was); } catch { }
                throw;
            }
            return Ok($"{kind} chart at {dstSheet}!{dstAddr} over {srcSheet}!{srcAddr} "
                      + $"({Math.Round(w).ToString(CultureInfo.InvariantCulture)}x{Math.Round(h).ToString(CultureInfo.InvariantCulture)}): "
                      + $"it REPLACED the {ChartKindName(was.Type)} chart that was already on those cells ({was.Series.Count} series) "
                      + "instead of stacking a second chart on it; undo puts the old data, type, title and size back" + note);

            static dynamic wbOf(dynamic shape) => shape.Parent.Parent;
        }
    }
}
