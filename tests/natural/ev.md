# Electric vehicles in Washington, 50 turns

File: `electric_vehicle_population_data.csv`, extracted from
`Electric_Vehicle_Population.zip` (Evals `dataframe/`). 181,458 real
registrations of electric and plug-in hybrid vehicles from the Washington
State Department of Licensing, model years 1997 to 2024. One conversation,
one prompt per message, in order.

What a careful analyst finds: `Electric Range` is 0 for 94,730 cars, which
means "not researched", not a range of zero (the CAFV column says so);
`Base MSRP` is 0 for 178,146, so price is missing for almost every car;
`Vehicle Location` is text like "POINT (-122.374105 47.54468)", longitude
first; `Electric Utility` packs several utilities into one cell with "|";
and 398 registrations are outside Washington.

## Excel

1. Open electric_vehicle_population_data.csv.
2. What is this data? A few sentences, and how many vehicles.
3. Save it as EV Registrations.xlsx and work in that.
4. Make the header row bold, freeze it, and fit the column widths.
5. How many are fully electric and how many are plug-in hybrids?
6. Which make is most common, and what share of all vehicles is it?
7. Make a Makes sheet: the number of vehicles for each make, largest first.
8. Add a column with each make's share of the total.
9. Only keep the top 15 makes in that table and group the rest as "Other".
10. Make a bar chart of the top 15 makes.
11. What's the average electric range? Look at the range column before you answer.
12. Lots of ranges are 0. Is that real? Check the eligibility column and tell me what the zeros mean.
13. Add a column that shows the range as blank where it wasn't researched, and use that from now on.
14. Average range by model year for fully electric cars, as a table on a new sheet called Range.
15. Make a line chart of that.
16. The Base MSRP column is almost all zeros. Can we say anything about price? Tell me; don't build anything.
17. Split the Vehicle Location column into separate Longitude and Latitude number columns.
18. Make a pivot of vehicle counts by county (rows) and vehicle type (columns).
19. Show only the top 10 counties, sorted.
20. Add a slicer for model year.
21. And a slicer for make.
22. Make a sheet called Growth with the number of vehicles by model year, 2010 onwards.
23. Add a column with the year-on-year growth.
24. Model year 2024 is much lower. Is that a real drop? Tell me what you think.
25. Add a note under the Growth table about 2024.
26. Make a column chart of vehicles by model year.
27. Change the chart colour to green and give it a proper title.
28. Which electric utility serves the most of these vehicles? Some cells list several utilities; deal with that sensibly.
29. Put the top 10 utilities in a table on a new sheet.
30. Rename the sheets: Data, Makes, Range, Counties, Growth, Utilities, in that order.
31. Build a Dashboard: the makes chart, the growth chart, the range line, and both slicers, with the total number of vehicles and the share that are fully electric as headline numbers.
32. Add a button that takes me to the Data sheet, and one that comes back to the Dashboard.
33. Some registrations aren't in Washington. How many, and which states? Put them on a sheet called Out of State.
34. Save the workbook.

## Word

35. Create a new Word document called EV Adoption in Washington.
36. Write a one-page briefing for a county transport planner.
37. Include the counties table and say which counties to plan charging for first.
38. Add the growth chart and explain the trend, including what's going on with 2024.
39. Add a section on range, explaining the zero values honestly.
40. Add a bulleted list of the limits of this data.
41. Make it less technical; the planner isn't a data person.
42. Add a header with the title and today's date, and save.

## PowerPoint

43. Create a new PowerPoint deck for the county council.
44. A title slide, then three slides: how many EVs, where they are, and how fast it's growing.
45. Put the charts in, one per slide.
46. Add a final slide with the decision we're asking the council to make.
47. Speaker notes on every slide.
48. The "where they are" slide is too crowded. Keep only the top five counties.
49. Move the decision slide to the front, right after the title.
50. Save it, and tell me the three numbers I should memorise before presenting.
