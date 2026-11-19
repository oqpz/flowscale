// Clean-room nearest-neighbor / integer-grid spatial upscaler (cs_5_0).
cbuffer UpscaleParams : register(b0)
{
    uint2 InputSize;
    uint2 OutputSize;
    uint4 DstRect; // x=offset_x, y=offset_y, z=dst_w, w=dst_h
    float Sharpness;
    uint ModeFlags; // bit 0: integer-centered sampling
    uint Subtype;
    uint VrsFlag;
};

Texture2D<float4> InputTex : register(t0);
RWTexture2D<float4> OutputTex : register(u0);

[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= OutputSize.x || tid.y >= OutputSize.y)
        return;

    if (tid.x < DstRect.x || tid.y < DstRect.y ||
        tid.x >= DstRect.x + DstRect.z || tid.y >= DstRect.y + DstRect.w)
    {
        OutputTex[tid.xy] = float4(0.0f, 0.0f, 0.0f, 1.0f);
        return;
    }

    uint2 localTid = tid.xy - DstRect.xy;
    uint2 dstSize = max(DstRect.zw, uint2(1, 1));

    if ((ModeFlags & 1u) != 0u)
    {
        uint scaleX = max(dstSize.x / max(InputSize.x, 1u), 1u);
        uint scaleY = max(dstSize.y / max(InputSize.y, 1u), 1u);
        uint intScale = max(min(scaleX, scaleY), 1u);
        uint2 scaled = InputSize * intScale;
        if (scaled.x <= dstSize.x && scaled.y <= dstSize.y)
        {
            uint2 pad = (dstSize - scaled) / 2u;
            if (localTid.x < pad.x || localTid.y < pad.y ||
                localTid.x >= pad.x + scaled.x || localTid.y >= pad.y + scaled.y)
            {
                OutputTex[tid.xy] = float4(0.0f, 0.0f, 0.0f, 1.0f);
                return;
            }
            int2 srcInt = int2((localTid - pad) / intScale);
            srcInt = clamp(srcInt, int2(0, 0), int2(InputSize) - 1);
            OutputTex[tid.xy] = InputTex.Load(int3(srcInt, 0));
            return;
        }
    }

    float2 uv = (float2(localTid) + 0.5f) / float2(dstSize);
    int2 srcCoord = int2(floor(uv * float2(InputSize)));
    srcCoord = clamp(srcCoord, int2(0, 0), int2(InputSize) - 1);

    OutputTex[tid.xy] = InputTex.Load(int3(srcCoord, 0));
}
