-
# Switch the active local STT engine. The engine names are awkward to say, so
# use short voice codes after the "use" prefix (the prefix guards against
# accidental switches mid-utterance):
#   use par / use para  -> parakeet
#   use q / use cue / use queue -> qwen
use (par | para): user.stt_select_engine("parakeet")
use (q | cue | queue): user.stt_select_engine("qwen")
