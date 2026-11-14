# The remote: a phone or tablet works the computer's Camera screen

A paired device, or one on a cable through `adb reverse`, can do what the
controls under the picture on the computer do: turn the face swap on and off,
choose the face, record a loop, and play it or go back to live. It also sees
the picture others see, as a small JPEG it can ask for a few times a second.

The desktop app hands its camera to the hub (`Hub::set_camera_remote` in
`core/gc-link`). Without it, as when the hub runs from the command line, every
route answers 404 `the camera is not available here`. Every route needs the
device's token, as the hub's other routes do; errors have the hub's usual
shape, `{"error": "...", "request_id": "..."}`.

| Route | Answer |
|---|---|
| `GET /v1/camera` | The state, below |
| `GET /v1/camera/faces` | `[{"id": "...", "name": "..."}]`, in the computer's order |
| `GET /v1/camera/faces/{id}/picture` | The face's small picture, PNG; 404 for a face that is not there |
| `GET /v1/camera/picture` | The picture others see, JPEG, no larger than 640 pixels; 404 before the first one |
| `POST /v1/camera` | One action as JSON, below; 204 when done, 409 with a code when not |

The state:

```json
{
  "mode": "live",               // "live", "looping" or "idle"
  "recording": false,
  "recorded_seconds": 0.0,
  "can_record": true,           // the picture is live
  "can_loop": false,            // there is a loop to play, or one plays
  "swap_wanted": false,
  "swapping": false,            // a swapped face is being drawn
  "swap_problem": null,         // when wanted and not on the face: a code, below
  "selected_face": "…",         // the face that would be shown
  "watched": true               // an app reads the camera: someone sees this picture
}
```

The actions, one per request:

```json
{"action": "swap", "on": true}
{"action": "face", "id": "…"}
{"action": "record"}            // start a recording, or end the one that runs
{"action": "loop"}              // play the loop, or go back to live
```

Codes are short words the device puts in its own language:

| Where | Code | Meaning |
|---|---|---|
| 409 | `accept_rules_on_computer` | The swap was never turned on at the computer, so its rules were never accepted there |
| 409 | `add_a_face_first` | No face is saved on the computer |
| 409 | `no_such_face` | The face asked for is not there any more |
| 409 | `cannot_record_now` | A recording starts only while the picture is live |
| 409 | `no_loop_yet` | There is no loop to play |
| `swap_problem` | `getting_ready` | The models are being loaded |
| `swap_problem` | `model_missing` | A model file is not installed on the computer |
| `swap_problem` | `failed` | The swap stopped with an error; the computer says which |
| `swap_problem` | `no_face_seen` | No face is in the picture |
| `swap_problem` | `not_on_face` | The swap is not on the face, and the picture is blurred |

The Windows app answers these from its model (`core/gc-app/src/remote.rs`).
