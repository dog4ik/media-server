import { exec } from "child_process";
import util from "util";
import fs from "fs";
type FileMetaData = {
  streams: {
    index: number;
    codec_name: string;
    codec_long_name: string;
    codec_type: string;
    duration: string;
    tags: { language: string };
  }[];
  format: {
    duration: string;
  };
};
type Response = {
  audioSuccess: boolean;
  subsSuccess: boolean;
  duration: number | null;
};
const EXTRACT_TOOL_PATH = "ffmpeg";
const INFO_TOOL_PATH = "ffprobe";

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
): Promise<Response> {
  let result: Response = {
    audioSuccess: false,
    subsSuccess: false,
    duration: null,
  };
  const metaDataQuery = `"${INFO_TOOL_PATH}" -v quiet -show_entries format=duration -print_format json -show_streams "${filePath}"`;
  const ffprobeResult = await asyncExec(metaDataQuery);
  if (!ffprobeResult.stdout)
    return { audioSuccess: false, subsSuccess: false, duration: null };
  const metaData = JSON.parse(ffprobeResult.stdout) as FileMetaData;
  if (metaData.format.duration) result.duration = +metaData.format.duration;

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
    console.log("audio ", track.codec_name);

    if (
      track.codec_name === "ac3" ||
      track.codec_name === "dts" ||
      track.codec_name === "eac3"
    ) {
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
