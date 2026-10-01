# US crude oil imports, 50 turns

File: `US_Crude_Oil_Import.csv` (Evals `dataframe/`). 483,053 rows of the
U.S. Energy Information Administration's monthly crude imports, January
2009 to January 2024: origin, destination, crude grade, quantity. One
conversation, one prompt per message, in order.

What a careful analyst finds: **the same oil is in the file several
times.** Each shipment is reported at four origin levels (World, OPEC or
non-OPEC, Region, Country) and at several destination levels (refinery,
port, their states, their PAD districts, the whole United States). Summing
every row for 2023 gives 65,943,430; counting it once, country to
refinery, gives 2,355,987, about 6.5 million barrels a day if the unit is
thousand barrels, which fits EIA's own figure. The unit is not in the file. And 2024 holds January only.

## Excel

1. Open US_Crude_Oil_Import.csv.
2. What does each row represent?
3. What unit is the quantity in? If the file doesn't say, tell me what you'd assume and why.
4. Save it as Crude Imports.xlsx and keep working in that.
5. Freeze the top row and make the headers bold.
6. Add a Date column built from the year and the month, as a real date.
7. What's the total quantity imported in 2023?
8. Are you sure that total doesn't count the same oil more than once? Look at the origin type and destination type columns.
9. Make a sheet called Countries to Refineries with only the rows where the origin is a single country and the destination is a refinery. Keep it as a table.
10. How many rows is that, and what's the 2023 total now?
11. Which five countries supplied the most oil over the whole period?
12. Make a Summary sheet with total imports per year, years down the side.
13. Add a column with the change from the year before, as a percentage.
14. Colour the increases green and the decreases red.
15. Add a line chart of imports per year, titled "US crude imports by year".
16. 2024 looks very low. Is that year complete? Check, and put a note beside the chart.
17. Make a pivot of quantity by origin country and year: countries down the side, years across.
18. Show only the top 10 countries in it.
19. Add a slicer for the crude grade.
20. Make a stacked column chart from the pivot showing the top countries over the years.
21. That chart is too busy. Make it show only Canada, Mexico and Saudi Arabia.
22. Which refineries take the most heavy sour crude? A top 10 table on a new sheet.
23. The refinery names are written as company / city / state. Split them into separate company, city and state columns.
24. Which states receive the most? Make a small table by state.
25. Make a bar chart of the top states.
26. Rename the Summary sheet to Trends.
27. Put the sheets in this order: Trends, the pivot, the refinery sheet, then the data.
28. Hide the original data sheet.
29. Actually, unhide it.
30. Name the yearly totals range YearlyImports.
31. Build a Dashboard with the yearly line chart, the three-country chart, the top states chart and the grade slicer. Make it fit on one screen.
32. Add a dropdown on the dashboard to pick a country, and show its total for each year next to it.
33. Add a button that resets the slicer.
34. Save.

## Word

35. Make a new Word document called "Where America's Crude Comes From".
36. Start with a one-paragraph summary of the trend since 2009.
37. Explain in plain words why the raw file counts the same oil several times, and how you avoided that.
38. Add the yearly totals table and the line chart.
39. Add a section on the top supplying countries, with the three-country chart.
40. Add a section on where the oil goes, states and refineries, with the tables.
41. Rewrite the summary for someone who knows nothing about oil.
42. Put the title in the header and page numbers in the footer, and save it.

## PowerPoint

43. Create a new PowerPoint deck for a ten-minute briefing on this.
44. A title slide, then one slide per section of the report.
45. Use the charts on the slides instead of tables.
46. Add a slide titled "The raw data counts oil twice", explaining it in three bullets.
47. Speaker notes on each slide, under 60 words each.
48. Remove the refineries slide; it's too detailed for this audience.
49. Add a final slide with the one number to remember.
50. Save the deck.
