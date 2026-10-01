# Global electricity, 50 turns

File: `Global_Electricity_Production.csv` (Evals `dataframe/`). 121,074 rows
of IEA monthly electricity statistics for 48 countries, January 2010 to
December 2023, in GWh: production by source, consumption, losses, imports
and exports. One conversation, one prompt per message, in order.

What a careful analyst finds: the products overlap. "Electricity" is the
total, "Total Renewables (Hydro, Geo, Solar, Wind, Other)" and "Total
Combustible Fuels" are subtotals, and the rest are their parts, so adding
products together counts the same power two or three times. Notes are mixed
in with the data: a parameter called "Remarks" and a product called "Data is
estimated for this month". Dates are text ("12/1/2023"). Coverage grows:
35 countries report production in 2010, 46 from 2015, 48 from 2021, so a
total across all countries is not like for like from year to year.

## Excel

1. Open Global_Electricity_Production.csv.
2. Describe what's in it in a few lines: which countries, which years, what's measured.
3. The date column is text. Make it a real date.
4. Save it as Electricity.xlsx and work in that.
5. Turn it into a table and freeze the header.
6. List the different parameters and products, and tell me which products are totals of other products.
7. Are there rows that aren't really data? Tell me what they are; don't delete anything yet.
8. Remove those rows.
9. Make a sheet called Production with only the net electricity production rows.
10. Add a Year column there.
11. How much electricity did all these countries produce together in 2022, in TWh?
12. Make a pivot: countries down the side, years across, total production.
13. Sort it so the biggest producer is at the top.
14. Add a slicer for the product.
15. Which country grew its production the most between 2012 and 2022? Add the growth as a column next to the pivot.
16. Make a Renewables sheet with each country's renewable share of production for each year.
17. Show the share as a whole-number percentage.
18. Put data bars on the 2022 column.
19. Which five countries had the highest renewable share in 2022, and which five the lowest?
20. Chart the renewable share over time for the top five, as lines.
21. Add the lowest five to the same chart as well.
22. Actually, take the lowest five off again. It's too cluttered.
23. For the United Kingdom, Germany and the United States, make a table of 2022 production from coal, natural gas, nuclear, wind, solar and hydro.
24. Turn that into a stacked bar chart, one bar per country.
25. Germany's nuclear looks small. Was there a change over these years? Tell me.
26. Make a small table of Germany's nuclear production per year, with a line chart next to it.
27. Show the numbers in that table in TWh instead of GWh, with one decimal.
28. Which month of the year is the peak for solar across all countries? Show it as a table of twelve months.
29. Colour the top three months in that table.
30. Is the total across all countries in 2010 comparable with 2023? Check how many countries report in each year and tell me.
31. Build a Dashboard: the renewable share lines, the three-country stacked bar, Germany's nuclear line, and the product slicer.
32. Add a dropdown to pick a country, and show its production by source for the latest full year next to it.
33. Add a button that refreshes everything.
34. Save.

## Word

35. Start a new Word document called "The Shift to Renewables".
36. Write an introduction: what the data is, where it's from, and what it can't tell us.
37. Add a section on renewable share, with the top and bottom five and the chart.
38. Add a section comparing the UK, Germany and the US, with the table and the stacked bar.
39. Add a short section on Germany's nuclear phase-out with its chart.
40. Add a note on why the all-countries totals shouldn't be compared across years.
41. Put the headings in a dark green and make the body text 11 point.
42. Add a table of contents and page numbers, and save it.

## PowerPoint

43. Make a new PowerPoint deck from the report for a lunchtime talk.
44. A title slide, an agenda slide, then one slide per report section.
45. Put the charts on the slides; no tables bigger than four rows.
46. Speaker notes for each slide.
47. Make the Germany slide a two-part comparison: before and after the phase-out.
48. Swap the order of the UK-Germany-US slide and the renewable share slide.
49. Add slide numbers.
50. Save it, and give me a one-sentence summary I could put in the meeting invite.
