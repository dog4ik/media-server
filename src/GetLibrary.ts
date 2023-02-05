import fs from "fs";
import path from "path";
import processFile from "./ProcessFile";
type LibItem = {
  title: string;
  episodes: {
    number: number;
    src: string;
    subSrc: string | null;
    duration: number | null;
  }[];
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
function getAllFiles(folder: string, files: string[]) {
  const filesInDir = fs.readdirSync(folder);
  for (const file of filesInDir) {
    const absolute = path.join(folder, file);
    if (fs.statSync(absolute).isDirectory()) {
      getAllFiles(absolute, files);
      continue;
    }
    files.push(file);
  }
  return files;
}
const getLibrary = async (folder: string) => {
  const library: LibItem[] = [];
  const scanDir = folder;
  let files: string[] = [];
  files = getAllFiles(folder, files);
  const getFilesRecursively = async (directory: string) => {
    const filesInDirectory = fs.readdirSync(directory);
    for (const file of filesInDirectory) {
      const absolute = path.join(directory, file);
      const relative = path.relative(scanDir, absolute);
      const regExp = /s[0-9]{1,2} ?e[0-9]{1,2}.+mkv$|mp4$/gi;
      if (fs.statSync(absolute).isDirectory()) {
        await getFilesRecursively(absolute);
      } else {
        const match = file.match(regExp);
        if (match) {
          const title = cleanTitle(file)
            .split(/s[0-9]{1,2} ?e[0-9]{1,2}/gi)[0]
            .trim()
            .toLowerCase();
          if (!title) return;
          const season = Number(
            file
              .match(/s[0-9]{1,2}/gi)![0]
              .toUpperCase()
              .replace("S", "")
          );
          const getIndexInLibrary = () =>
            library.findIndex(
              (item) => item.title == title && item.season == season
            );
          if (getIndexInLibrary() == -1) {
            library.push({
              episodes: [],
              title,
              season,
            });
          }
          console.log("started working on: " + file);
          let subsPath = null;

          const fileProcessResult = await processFile(absolute, file, files);
          if (fileProcessResult.subsSuccess) {
            subsPath =
              "/" +
              relative.replaceAll("\\", "/").replace(/.mkv$|.mp4$/g, ".srt");
          }
          console.log("finished working on: " + file);
          if (fileProcessResult.audioSuccess) {
            library[getIndexInLibrary()].episodes.push({
              number: Number(
                file
                  .match(/e[0-9]{1,2}/gi)![0]
                  .toUpperCase()
                  .replace("E", "")
              ),
              src: "/" + relative.replaceAll("\\", "/"),
              subSrc: subsPath,
              duration: fileProcessResult.duration,
            });
          }
        }
      }
    }
  };
  await getFilesRecursively(folder);
  return library;
};

export default getLibrary;
