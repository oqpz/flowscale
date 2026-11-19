// Clean-room edge-directed diagonal pixel-art scaler (cs_5_0).
cbuffer UpscaleParams : register(b0)
{
    uint2 InputSize;
    uint2 OutputSize;
    uint4 DstRect;
    float Sharpness;
    uint ModeFlags;
    uint Subtype;
    uint VrsFlag;
};

Texture2D<float4> InputTex : register(t0);
RWTexture2D<float4> OutputTex : register(u0);

float ColorDist(float3 a, float3 b)
{
    float3 d = abs(a - b);
    return dot(d, float3(0.299f, 0.587f, 0.114f)) + max(d.r, max(d.g, d.b)) * 0.25f;
}

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

    float2 pos = (float2(localTid) + 0.5f) * float2(InputSize) / float2(dstSize);
    int2 center = clamp(int2(floor(pos)), int2(0, 0), int2(InputSize) - 1);
    float2 f = frac(pos) - 0.5f; // [-0.5, 0.5] inside source texel

    int sx = (f.x >= 0.0f) ? 1 : -1;
    int sy = (f.y >= 0.0f) ? 1 : -1;
    float2 af = abs(f);

    int2 maxC = int2(InputSize) - 1;
    float4 cE  = InputTex.Load(int3(center, 0));
    float4 cH  = InputTex.Load(int3(clamp(center + int2(sx, 0),  int2(0, 0), maxC), 0));
    float4 cV  = InputTex.Load(int3(clamp(center + int2(0, sy),  int2(0, 0), maxC), 0));
    float4 cD  = InputTex.Load(int3(clamp(center + int2(sx, sy), int2(0, 0), maxC), 0));
    float4 cHo = InputTex.Load(int3(clamp(center + int2(-sx, 0), int2(0, 0), maxC), 0));
    float4 cVo = InputTex.Load(int3(clamp(center + int2(0, -sy), int2(0, 0), maxC), 0));

    // Measure diagonal continuity vs center-corner difference in the active quadrant.
    float dHV = ColorDist(cH.rgb, cV.rgb);
    float dED = ColorDist(cE.rgb, cD.rgb);
    float dEH = ColorDist(cE.rgb, cH.rgb);
    float dEV = ColorDist(cE.rgb, cV.rgb);
    float dHHo = ColorDist(cH.rgb, cHo.rgb);
    float dVVo = ColorDist(cV.rgb, cVo.rgb);

    // Favor blending the diagonal corner edge if H and V match closely while E differs from H/V.
    float diagSupport = (dED + dHHo + dVVo) - (dHV * 2.0f + (dEH + dEV) * 0.5f);
    float4 outColor = cE;

    if (dHV < 0.12f && (dEH > 0.04f || dEV > 0.04f) && diagSupport > 0.0f)
    {
        // Corner triangle: pixels beyond (af.x + af.y > 0.5) cross the 45-degree edge.
        float dist = (af.x + af.y) - 0.5f;
        float aa = saturate(dist * 3.0f + 0.5f);
        float4 corner = (dEH <= dEV) ? cH : cV;
        outColor = lerp(cE, corner, aa);
    }

    OutputTex[tid.xy] = outColor;
}
