import express from "express";
import path from "path";
import fs from "fs";
import cors from "cors";
import dotenv from "dotenv";
import getLibrary from "./src/GetLibrary";
dotenv.config();
const app = express();
const scanDir = process.env.SCAN_DIR ?? "";
app.use(cors());
app.use("/static", express.static(scanDir));
app.get("/show", async (_, res) => {
  res.send(await getLibrary(scanDir));
});
app.get("/test", (_, res) => {
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
  let files: string[] = [];
  res.send(getAllFiles("S:\\video\\show", files));
});
app.listen(process.env.PORT, () => {
  console.log(
    `[server]: Server is running at https://localhost:${process.env.PORT}`
  );
});
