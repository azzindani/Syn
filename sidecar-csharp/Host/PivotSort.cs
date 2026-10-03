// Sorting a range that is a PivotTable.
//
// Range.Sort cannot touch a pivot: Excel answers "Unable to get the Sort
// property of the Range class", which says nothing about what to do instead,
// and a run that was asked to "sort it so the biggest producer is at the top"
// gave up on the pivot altogether and rebuilt it as a grid of SUMIFS. A person
// sorts a pivot through its row field: by the totals, by the labels, or by one
// column of it. All three are PivotField.AutoSort, and what `sort` does when
// its range is inside a pivot.

using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        private sealed class PivotSortState
        {
            public string Sheet = "", Pivot = "", RowField = "", SortField = "";
            public int Order;
            public object? Line;
        }

        /// <summary>The PivotTable a range starts in, or null for plain cells.</summary>
        private static dynamic? PivotAt(dynamic rng)
        {
            try { return rng.Cells[1, 1].PivotTable; }
            catch (Exception) { return null; }
        }

        /// <summary>The sort a pivot has now, kept for `undo`, when a `sort`
        /// call's range is inside a pivot; null otherwise.</summary>
        private static PivotSortState? CapturePivotSort(dynamic wb, string method, string selector)
        {
            if (method != "sort") return null;
            try
            {
                var t = Target((object)wb, selector, "sort");
                dynamic? pivot = PivotAt((object)t.rng);
                if (pivot == null) return null;
                dynamic rf = pivot.RowFields(1);
                var st = new PivotSortState
                {
                    Sheet = (string)t.sheet, Pivot = (string)pivot.Name, RowField = (string)rf.Name,
                    Order = (int)rf.AutoSortOrder,
                };
                try { st.SortField = (string)rf.AutoSortField; } catch (Exception) { }
                try { st.Line = rf.AutoSortPivotLine; } catch (Exception) { }
                return st;
            }
            catch (Exception) { return null; }
        }

        private static void RestorePivotSort(dynamic book, PivotSortState st)
        {
            dynamic rf = Sheet(book, st.Sheet).PivotTables(st.Pivot).RowFields(st.RowField);
            var field = st.SortField.Length > 0 ? st.SortField : st.RowField;
            if (st.Line != null)
            {
                try { rf.AutoSort(st.Order, field, st.Line); return; }
                catch (COMException) { }
            }
            rf.AutoSort(st.Order, field);
        }

        /// <summary>`sort` on a range inside a PivotTable: its rows by the
        /// totals (name empty, "Grand Total" or the data field), by their own
        /// labels (name is the row field), or by one of its columns (name is
        /// that column's heading, e.g. a year).</summary>
        private static string SortPivot(dynamic pivot, string sheet, string header, bool desc)
        {
            const int xlAscending = 1, xlDescending = 2;
            int order = desc ? xlDescending : xlAscending;
            if ((int)pivot.RowFields().Count < 1)
                throw new InvalidOperationException("sort: this PivotTable has no row field to sort; give it one with `pivot` rows");
            dynamic rf = pivot.RowFields(1);
            var rowName = (string)rf.Name;
            var dataName = (string)pivot.DataFields(1).Name;
            var h = (header ?? "").Trim();
            var dataNames = new List<string>();
            for (var i = 1; i <= (int)pivot.DataFields().Count; i++) dataNames.Add((string)pivot.DataFields(i).Name);

            string by;
            if (h.Length == 0 || h.Equals("Grand Total", StringComparison.OrdinalIgnoreCase) || dataNames.Contains(h, StringComparer.OrdinalIgnoreCase))
            {
                var field = dataNames.FirstOrDefault(n => n.Equals(h, StringComparison.OrdinalIgnoreCase)) ?? dataName;
                rf.AutoSort(order, field);
                by = $"its totals ({field})";
            }
            else if (h.Equals(rowName, StringComparison.OrdinalIgnoreCase) || h.Equals("Row Labels", StringComparison.OrdinalIgnoreCase))
            {
                rf.AutoSort(order, rowName);
                by = $"its row labels ({rowName})";
            }
            else
            {
                dynamic? line = null;
                var heads = new List<string>();
                try
                {
                    dynamic lines = pivot.PivotColumnAxis.PivotLines;
                    for (var i = 1; i <= (int)lines.Count; i++)
                    {
                        string? name = null;
                        try { name = (string)lines.Item(i).PivotLineCells.Item(1).PivotItem.Name; } catch (Exception) { }
                        if (name == null) continue;
                        heads.Add(name);
                        if (line == null && name.Equals(h, StringComparison.OrdinalIgnoreCase)) line = lines.Item(i);
                    }
                }
                catch (Exception) { }
                if (line == null)
                    throw new InvalidOperationException(
                        $"sort: a PivotTable sorts its rows by its totals (name empty or Grand Total), by their labels (name {rowName}) " +
                        (heads.Count > 0 ? $"or by one of its columns ({string.Join(", ", heads.Take(20))}); " : "; ") +
                        $"{h} is none of those");
                rf.AutoSort(order, dataName, line);
                by = $"its {h} column";
            }
            return Ok($"sorted the PivotTable {(string)pivot.Name} on {SheetRef(sheet)} by {by}, {(desc ? "largest" : "smallest")} first; " +
                      $"its rows come from {rowName}, so the order holds when the data is refreshed");
        }
    }
}
