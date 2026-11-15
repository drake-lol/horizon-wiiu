; $MODE = "UniformRegister"

; numOutputs - 1
; $SPI_VS_OUT_CONFIG.VS_EXPORT_COUNT = 1

; $NUM_SPI_VS_OUT_ID = 1

; colour -> semantic 0, texture coordinate -> semantic 1
; $SPI_VS_OUT_ID[0].SEMANTIC_0 = 0
; $SPI_VS_OUT_ID[0].SEMANTIC_1 = 1
; $SPI_VS_OUT_ID[0].SEMANTIC_2 = 255
; $SPI_VS_OUT_ID[0].SEMANTIC_3 = 255

; C0
; $UNIFORM_VARS[0].name = "uModelViewProj"
; $UNIFORM_VARS[0].type = "mat4"
; $UNIFORM_VARS[0].count = 1
; $UNIFORM_VARS[0].block = -1
; $UNIFORM_VARS[0].offset = 0

; R1
; $ATTRIB_VARS[0].name = "aPosition"
; $ATTRIB_VARS[0].type = "vec4"
; $ATTRIB_VARS[0].location = 0
; R2
; $ATTRIB_VARS[1].name = "aColor"
; $ATTRIB_VARS[1].type = "vec4"
; $ATTRIB_VARS[1].location = 1
; R3
; $ATTRIB_VARS[2].name = "aTexCoord"
; $ATTRIB_VARS[2].type = "vec4"
; $ATTRIB_VARS[2].location = 2

; Multiply uModelViewProj and aPosition

00 CALL_FS NO_BARRIER
01 ALU: ADDR(32) CNT(16)
    0  x: MUL    ____, R1.w, C3.x
       y: MUL    ____, R1.w, C3.y
       z: MUL    ____, R1.w, C3.z
       w: MUL    ____, R1.w, C3.w
    1  x: MULADD R127.x, R1.z, C2.x, PV0.x
       y: MULADD R127.y, R1.z, C2.y, PV0.y
       z: MULADD R127.z, R1.z, C2.z, PV0.z
       w: MULADD R127.w, R1.z, C2.w, PV0.w
    2  x: MULADD R127.x, R1.y, C1.x, PV1.x
       y: MULADD R127.y, R1.y, C1.y, PV1.y
       z: MULADD R127.z, R1.y, C1.z, PV1.z
       w: MULADD R127.w, R1.y, C1.w, PV1.w
    3  x: MULADD R1.x, R1.x, C0.x, PV2.x
       y: MULADD R1.y, R1.x, C0.y, PV2.y
       z: MULADD R1.z, R1.x, C0.z, PV2.z
       w: MULADD R1.w, R1.x, C0.w, PV2.w
02 EXP_DONE: POS0, R1
03 EXP: PARAM0, R2
04 EXP_DONE: PARAM1, R3 NO_BARRIER
END_OF_PROGRAM
