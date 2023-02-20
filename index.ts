import express from "express";
import cors from "cors";
import dotenv from "dotenv";
import getLibrary from "./src/GetLibrary";
dotenv.config();
const app = express();
const scanDir = process.env.SCAN_DIR!;
app.use(cors());
app.use("/static", express.static(scanDir));
app.get("/show", async (_, res) => {
  res.send(await getLibrary(scanDir));
});
app.get("/test", async (req, res) => {
  res.json(req.socket.remoteAddress);
});
app.listen(process.env.PORT, () => {
  console.log(
    `[server]: Server is running at https://localhost:${process.env.PORT} \n Selected dir: ${process.env.SCAN_DIR}`
  );
});
