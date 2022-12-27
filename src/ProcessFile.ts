import { exec } from "child_process";
import util from "util";
import fs from "fs";
import path from "path";
type FileMetaData = {
  streams: {
    index: number;
    codec_name: string;
    codec_long_name: string;
    codec_type: string;
    duration: string;
    tags: { language: string };
  }[];
};
const EXTRACT_TOOL_PATH = path.join(
  "C:",
  "Program Files",
  "Ffmpeg",
  "bin",
  "ffmpeg.exe"
);
const INFO_TOOL_PATH = path.join(
  "C:",
  "Program Files",
  "Ffmpeg",
  "bin",
  "ffprobe.exe"
);
const asyncExec = util.promisify(exec);

const encodeAudio = async (filePath: string, desiredTrackId: number) => {
  const tempPath = filePath + "buffer";
  fs.rename(filePath, tempPath, () => {
    console.log("\nFile Renamed!\n");
  });
  const encodeQuery = `"${EXTRACT_TOOL_PATH}" -i "${tempPath}" -map 0:v:0 -map 0:${desiredTrackId} -acodec aac -vcodec copy "${filePath}"`;
  console.log(encodeQuery);

  await asyncExec(encodeQuery).then(() => {
    fs.unlink(tempPath, (err) => {
      if (err) throw err;
      console.log(`\n${tempPath} was deleted\n`);
    });
  });
};

export default async function prosessFile(
  filePath: string,
  fileName: string,
  allFiles: string[]
) {
  let result = {
    audioSuccess: false,
    subsSuccess: false,
  };
  const infoQuery = `"${INFO_TOOL_PATH}" -v quiet -print_format json -show_streams "${filePath}"`;
  const mkvdata = await asyncExec(infoQuery).catch((e) => {
    console.log(e);
    return e;
  });
  if (!mkvdata.stdout) return { audioSuccess: false, subsSuccess: false };
  const metaData = JSON.parse(mkvdata.stdout) as FileMetaData;

  //handle Subtitles
  let desiredTrackId: null | number = null;
  for (let i = 0; i < metaData.streams.length; i++) {
    const track = metaData.streams[i];
    if (
      track.codec_type !== "subtitle" ||
      track.tags.language !== "eng" ||
      track.codec_name !== "subrip"
    )
      continue;
    desiredTrackId = track.index;
  }
  console.log(desiredTrackId);
  if (
    desiredTrackId !== null &&
    !allFiles.includes(fileName.replace(/.mkv$|.mp4$/g, ".srt"))
  ) {
    const createSubsQuery = `"${EXTRACT_TOOL_PATH}" -i "${filePath}" -map 0:${desiredTrackId} "${filePath.replace(
      /.mkv$|.mp4/g,
      ".srt"
    )}" -y`;
    await asyncExec(createSubsQuery)
      .then(() => {
        result.subsSuccess = true;
      })
      .catch(() => {
        console.log("Error creating subtitles for: " + filePath);
        result.subsSuccess = false;
      });
  }
  if (allFiles.includes(fileName.replace(/.mkv$|.mp4$/g, ".srt")))
    result.subsSuccess = true;

  //handle audio codec
  for (let i = 0; i < metaData.streams.length; i++) {
    const track = metaData.streams[i];
    if (track.codec_type !== "audio" || track.tags.language !== "eng") continue;
    if (track.codec_name === "aac" || track.codec_name === "mp3") {
      result.audioSuccess = true;
      continue;
    }
    if (track.codec_name === "ac3" || track.codec_name === "dts") {
      await encodeAudio(filePath, track.index)
        .then(() => {
          result.audioSuccess = true;
        })
        .catch(() => {
          console.log("error while prosessing audio from: " + filePath);
          result.audioSuccess = false;
        });
      break;
    }
  }
  console.log(result);

  return result;
}
