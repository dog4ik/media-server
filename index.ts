import express from "express";
import cors from "cors";
import path from "path";
import fs from "fs";
type LibItem = {
  title: string;
  episodes: { number: number; src: string }[];
  season: number;
};
function cleanTitle(rawTitle: string) {
  let cleanedTitle = rawTitle;
  if (cleanedTitle.indexOf(".") !== -1) {
    cleanedTitle = cleanedTitle.replace(/\./g, " ");
  }

  cleanedTitle = cleanedTitle.replace(/_/g, " ");
  cleanedTitle = cleanedTitle.replace(/([(_]|- )$/, "").trim();

  return cleanedTitle;
}
const scanDir = "S://video//show";
const getLibrary = (folder: string) => {
  const library: LibItem[] = [];
  const getFilesRecursively = (directory: string) => {
    const filesInDirectory = fs.readdirSync(directory);
    for (const file of filesInDirectory) {
      const absolute = path.join(directory, file);
      const relative = path.relative(scanDir, absolute);
      const regExp = /s[0-9]{1,2} ?e[0-9]{1,2}.+mkv|mp4/gi;
      if (fs.statSync(absolute).isDirectory()) {
        getFilesRecursively(absolute);
      } else {
        const match = file.match(regExp);
        if (match) {
          const title = cleanTitle(file)
            .split(/s[0-9]{1,2} ?e[0-9]{1,2}/gi)[0]
            .trim()
            .toLowerCase();
          const season = Number(
            file
              .match(/s[0-9]{1,2}/gi)![0]
              .toUpperCase()
              .replace("S", "")
          );

          if (
            library.findIndex(
              (item) => item.title == title && item.season == season
            ) == -1
          ) {
            library.push({
              episodes: [
                {
                  number: Number(
                    file
                      .match(/e[0-9]{1,2}/gi)![0]
                      .toUpperCase()
                      .replace("E", "")
                  ),
                  src: "/" + relative.replace("\\", "/"),
                },
              ],
              title,
              season,
            });
          } else {
            library[
              library.findIndex(
                (item) => item.title == title && item.season == season
              )
            ].episodes.push({
              number: Number(
                file
                  .match(/e[0-9]{1,2}/gi)![0]
                  .toUpperCase()
                  .replace("E", "")
              ),
              src: "/" + relative.replace("\\", "/"),
            });
          }
        }
      }
    }
  };
  getFilesRecursively(folder);
  return library;
};
const app = express();
app.use(cors());
app.use("/static", express.static(scanDir));
app.get("/show", (_, res) => {
  res.send(getLibrary(scanDir));
});
app.listen(3001, () => {
  console.log(`[server]: Server is running at https://localhost:${3001}`);
});
